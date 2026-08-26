> Historical. This document predates the chat removal (2026-08-20) and describes conversion planning against a tree that still had chat. Treat plated/deferred status as of that snapshot, not current.

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

## `P-peer-exchange`

🆕 **Added 2026-08-19 by operator ruling.** Journal↔journal exchange between two instances the same owner holds — covers `transfer` and `export`. ⛔ **Not egress** — the far end is a journal the same owner holds, not a third party; see § *Egress — where the covenant applies*.

Two cross-instance contracts that had no owner get one here — see [`strands.md`](strands.md) § `S:journal-segment:peer-exchange` · § `S:device-link:peer-exchange`:

- **Archive manifest v1** — the durable format `transfer export` writes and `transfer import` reads.
- **The peer-ingest HTTP surface**, `/app/import/journal/{prefix}/…`.

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
`P-journal-retention` marks raw media on it for removal · `P-device-ingest` proves to a device that its
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

**Shape, re-measured 2026-08-13 — the plate is NATIVE.** One long-running dispatcher plus three
handlers, all separate processes. ⛔ `observe/sense.py` and `observe/describe.py` **no longer exist**;
both were deleted from `main` on 2026-08-13 .

| | Module | Reads | Writes | Notes |
|---|---|---|---|---|
| dispatcher | **`solstone-core-sense`** (library crate, wired as the `solstone-core sense` subcommand) | the `observe.observing` bus event, or a `--day` scan | nothing in the segment | spawns handlers by **file extension**, one worker pool per handler, memory-gated, per-job wall-clock caps (`describe` 1800s · `transcribe` 2700s · `depict` 600s) |
| audio | `observe/transcribe/` (~3,100) | `.flac .opus .ogg .m4a .mp3 .wav` | `<stem>.jsonl`, `<stem>.npz` | VAD → silence reduction → backend registry → STT → native speaker analysis |
| screen | **`solstone-core-describe`** (sibling binary, 7,221 lines) | `.webm .mp4 .mov` | `<stem>.jsonl` | dHash winnow → ArUco mask → categorize → select → extract. ⚠ **Linux-only** — see below |
| image reference | `observe/depict.py` (104) | `.png .jpg .jpeg .heic .heif .gif .webp .tiff` | `<stem>.jsonl` | frozen oracle for the native handler |

🍎 **`describe` ships Linux-only, deliberately.** A macOS journal host has no `journal describe`: it
dispatches to a sibling not installed there and exits **70**, which `sense` records as a segment error
and notifies. Holding the cut would have stranded a landed handler behind a signing credential no
session can obtain. ⚠ The mac wheel **builds**; only signing is missing.

⚠ **The dispatcher spawns the sibling `solstone-core-journal`** (`[<abs>/solstone-core-journal, "describe", …]`),
resolved from the running executable. `PATH` is not consulted; a missing or non-executable sibling fails
closed as a per-file segment error naming the candidate path.

🔴 **The dispatcher is the plate.** Its behaviour is not incidental: skip and defer gates, re-entry rules, the memory gate, the watchdog, `exit 69` hold-raw, and segment completion all live there, and none of it is in a handler. ⚠ **Two more that were never written down and the native port carries:** the **deferred / no-engine gate** — for a *live* (non-batch) segment, `mode == "deferred"` or no thinking engine chosen means no file is tracked, **no handler spawns**, and `observed` is emitted immediately with note `deferred` / `no_engine`, which is the owner's don't-process-live switch — and **`<day>/health/stream.updated`**, touched on completion for **live segments only**, with named downstream consumers; ⛔ batch (re-process / importer) segments must not advance it. ⚠ `observe/{hear,screen,see,grab,pdf_worker}.py` (2,269 lines) carry `observe/` names but are **read-side or other plates entirely** — sense reaches none of them.

⚠ **Handler exit codes are a contract of their own** and `observe/exit_codes.py` declares only part of it: `EXIT_PROVIDER_BLOCKED = 69`, plus `WATCHDOG_TIMEOUT`, which despite the module name is **a log string compared against nothing**. Also live and undeclared: **1** (transcribe hard failure and speaker-analysis failure), and **78** — ⚠ which is *not* a handler code at all but the dispatcher's own startup exit (`sense.py:1423`), before any handler runs.

🔴 **A code CHANGED MEANING on 2026-08-13** . The dispatcher's result for an
**unresolvable native sibling** moved from **69 to 70** across all native tokens, so **69 now means
only** *"a handler ran and hit an owner-remediable provider condition."* ⚠ The reason is the one that
matters: at 69 the dispatcher reads a missing sibling as an honest deferral — no error recorded, no
notification, input left in place, re-picked forever — so a verb genuinely unavailable on a platform
looks identical to a busy provider. **70 surfaces; 69 defers.** ⛔ Do not re-collapse them.
📌 Three greppable operator diagnostics distinguish the causes, since the exit code no longer does:
`native-helper-missing:` · `native-helper-not-executable:` · `native-helper-current-exe:`.

🔴 **⛔ `sense` exits 0 and prints a success banner when EVERY handler fails to spawn** — measured
2026-08-13. Its `observed` event correctly carries `error: true` and a populated `errors` list, so the
plate contract holds; the CLI discards it. ⚠ Compounded by `solstone-core` installing no logger, which
leaves the `ManagedProcess` log for a failed handler at **zero bytes**. ⚠ Related: `describe` discards a *specific* refusal reason on its own blocked path — the reason
is carried to `RunError::Blocked`, which is a unit variant that drops it.

🔴 **CORRECTED 2026-08-05 — "a deferral records neither success nor failure" is FALSE, and the comment that says so is in the code.** `sense.py:549-560` states the intent and does not implement it. The `69` branch and the `exit 0` branch **both** call `_check_segment_observed(file_path)`; the *only* difference is that success additionally calls `_record_successful_contact()`, a health-beacon counter — and that same counter is also ticked by the idle status loop every 5 seconds (`:885-887`), so it distinguishes nothing durable either.

⛔ **Consequence: a deferred segment is emitted as `observe.observed` with no error field, indistinguishable from a cleanly processed one**, and `stream.updated` is touched on the live path. Every downstream consumer — `think/top.py:274`, `think/supervisor.py:5919`, `think/importers/cli.py:164`, `apps/events.py:13` — proceeds as though the media was processed when it was not. ⚠ The hold-raw half is real and works: no output is written and the input is left in place, so the next scan re-picks the file. **It is the announcement that lies, not the retention.**

📌 **What a rebuild must anchor on instead:** no output written · the dispatcher does not unlink · **no `observed` emission that a consumer can mistake for a completed segment** · the file is re-selected on the next scan. ⛔ Do not port the code comment; port the corrected behaviour, and note that the same wording appears as a carry-forward on `S:segment-media:journal-segment` in [`strands.md`](strands.md).

⚠ **Two more undeclared behaviours in the same file.** Segment identity in every tracking structure is the **bare `HHMMSS_LEN` key**, not `(day, stream, segment)` — so two streams whose segments start in the same second collide, merging pending sets and landing errors on the wrong segment. And **shutdown records in-flight work as terminal failure** (`_run_handler:573-584`): a SIGTERM'd handler's non-zero exit becomes a segment error and emits `observed` with `error: True` — ⚠ and the daily repair phase runs the batch dispatcher under a wall-clock budget (`think/thinking.py:4611`), so a phase that runs over systematically writes **false failures**.

🔴 **Two historical silent-success paths.** Retired `describe.py:964-967` and `depict.py:64-69` **returned exit 0 having written nothing** when no thinking engine was configured, and the dispatcher read that as success — it recorded a successful contact and marked the file done (`sense.py:562-571`). The live path was protected by a gate (`sense.py:817-825`); ⚠ **the `--day` batch path was not**, and the daily sense-repair pre-phase (`think/thinking.py:4592-4632`) was exactly that path. Re-entry eventually recovered because no output existed, but the success signal was false while it did.

🆕 🔴 **MEASURED 2026-08-09 by running the handler: `transcribe` has TWO sites that unlink the owner's raw audio, not one, and they are 440 lines apart.**

| site | fires when | state / reason written first |
|---|---|---|
| `transcribe/main.py:1255` | VAD reports insufficient speech, **before** STT runs | `empty` / `no_decodable_audio` |
| `transcribe/main.py:815` | **STT returned zero statements**, after VAD accepted the clip | `empty` / `no_decodable_audio` |

Both are gated only by `transcribe.preserve_all`, which **defaults to false**, and both write a terminal processing record *before* unlinking — ✅ the correct fail-closed order, and if that write fails the handler raises and never unlinks.

⚠ **The second site is reachable on audio VAD accepted.** Observed: a clip scored `3.4s speech, has_speech=True` went to STT, the engine returned no words, and the raw was removed. **So an STT backend answering `200` with an empty word list deletes the owner's audio and records it as `no_decodable_audio`** — indistinguishable in the durable record from a clip that genuinely had no speech.

⚠ **`_audio_wire.parse_words` is what separates the two outcomes**, and the razor is narrow: `{"words": [], "text": ""}` reaches the delete; `{"words": [], "text": "hello"}` raises a contract error, writes nothing, and **preserves the audio.**

📌 There is a **third** unlink in the same write path — `transcribe/native.py:183-197` removes an existing `.npz` whenever its `.jsonl` is absent, on every write call. Derived artifact, not owner media.

⛔ **Recorded as a measurement, not a proposal.** `P-journal-retention` § 2 says *"`transcribe` stops unlinking VAD-empty raw audio"*; read literally that names one site and leaves the other in place. **Whether the ruling covers both remains unresolved and is not edited here.**

⚠ **The retry budget is describe-only in practice.** `should_reenter_analysis_output` (`observe/processing_record.py:118-152`) returns `True` **only** for `handler == "describe"`, and transcribe writes its `corrupt_input` output through `_write_failed_processing_jsonl`, which then blocks re-entry at three separate guards. `FAILED_ATTEMPT_BOUND` never applies to audio.

**What is already Rust:** the speaker math, behind a one-record argv+stdio contract: `solstone-core-speakers` (3,749), `-speakers-analyze` (2,049), `-speakers-onnx` (662), reached through `solstone/observe/transcribe/speakers_analyze_adapter.py`. `NATIVE_PROCESS_SPECS` routes `journal depict` to the `solstone-core-depict` binary; its entry point calls the handler in `core/crates/solstone-core-depict/src/lib.rs`. The handler reads and verifies the RF-DETR sidecar and pinned artifacts in Rust. `journal_native_dispatch.rs` poisons sibling interpreters while exercising the compiled journal dispatch, and the depict crate tests the native RF-DETR query. The dispatcher and describe/transcribe drivers remain mapped to their Python owner modules; the native depict handler now writes a `_solstone_processing` header record and re-enters through `should_reenter_analysis_output`, like describe.

**Packaging:** `packages/solstone-core-depict/pyproject.toml` builds the native depict binary with Maturin. The CPU and CUDA journal manifests each pin that package for their supported native platforms, so the sibling binary is installed with the `journal depict` route. The release inventory derives required native packages from Cargo default binaries and their UV/Maturin mappings; `make check-release-package-inventory` fails when a required binary has no package.

🆕 ⚠ **`describe` DOES write a processing record** — it stamps one at every terminal promote, including `attempts` on failures, and `should_reenter_analysis_output` is keyed on it. **`depict` wrote none in Python**; `solstone-core-depict` now writes one. A still image still cannot be proven consumed for retention — because `expected_handler` returns `None` for image extensions (the closed set does not claim them), not because a record is absent.

🆕 🔴 **Truncation is invisible to this plate.** Measured against a reference-observed corpus (`core/fixtures/describe_frames.json`): a WebM cut short by a crashed recorder decodes **cleanly** to a shorter frame set with no decode-failure flag, so the handler records `analyzed` / `ok` over a partial description and nothing anywhere says frames were lost. Corruption early in the stream does set the flag, and yields nothing. ⚠ The reference's branch that returns already-collected frames *alongside* a decode failure was unreachable across a sweep of 46 corruption offsets at two widths — it is unpinned by any corpus.

🆕 ⚠ **The frame loop's order is not the obvious one, and a rebuild that gets it wrong stays green.** Per decoded frame: the `raw` counter increments **before** the presentation-timestamp check; the fiducial mask runs **before** the perceptual hash, so the hash is computed on the *masked* image; and a frame the mask rejects consumes its frame index without advancing the winnow's last-kept reference. ⛔ A corpus carrying no fiducials cannot detect the mask being applied after the winnow instead of before the hash.

## `P-index`

🔴 **`day` semantics — one meaning, not three.** `day` is **the day the content originated from**: the source segment's day, or for an activity its **start** time. ⛔ It is not the recording day, not the last-seen day, and not the ingest day. For content that is genuinely not day-based, the **only** permitted fallback is the day it was last updated, and a fallback must be named as one rather than silently occupying the same field. ⚠ Before this, `day` conflated recording, source and last-seen meanings.

The SQLite index. **Ephemeral by design and always rebuildable — that property is required, not incidental.**

**Production mutation authority:** `journal indexer` dispatches in the Rust journal binary, and `solstone-core-indexer-store` owns scans, file replacements, stream/path pruning, resets, edge rebuilds, and entity-merge edge maintenance. Python feature plates that have not converted yet request those explicit native mutations rather than opening SQLite for writes. The [poisoned-path integration test](../../core/crates/solstone-core-journal-bin/tests/journal_identity.rs) exercises a real rebuild while the supported Python launchers fail on invocation. `think/indexer/journal.py` and `edges.py` retain their Python-era implementations as differential references.

**Schema authority:** the production DDL lives in Rust; the Python DDL is reference corpus. Before a native mutation, Rust transactionally copies pre-`stream` and pre-`time_bucket` FTS rows into the current table shape. The [legacy-shape tests](../../core/crates/solstone-core-indexer-store/src/db.rs) cover both migrations and assert that the existing rows remain queryable.

**Schema v2 sequencing:** v2 remains the next schema change and is separate from this authority change, so rollback does not also have to undo a new durable shape. Its minimum is **SQLite 3.42**, the version that introduced the FTS5 [`secure-delete` configuration option](https://www.sqlite.org/fts5.html#the_secure_delete_configuration_option). The workspace [builds `rusqlite` with bundled SQLite](../../core/Cargo.toml); the iOS canary excludes indexer-store/query and therefore does not set the journal-host floor.

**Shape of the live schema:** one FTS5 virtual table (`content` + **seven `UNINDEXED` columns** — `path`, `day`, `facet`, `agent`, `stream`, `idx`, `time_bucket`), a `files(path, mtime)` staleness watermark, and the derived `edges` / `edge_files` pair. 🔴 **Every metadata filter is therefore a post-filter over the whole match set, and a filter with no search term is a full table scan** — `_build_where_clause` emits `1=1` for an empty query. The `edges` half, which does have real indices, is the existing proof the same file can serve indexed lookups.

**Carry forward — measured on a large populated journal (2.83M chunk rows, 1.64 GB, 439 days):**

- 🔴 **FTS5 `optimize` is never run anywhere in either implementation, and the scheduler does not run it.** On a corpus with ~98k write transactions this left **34% of the file** as unmerged-segment fragmentation: the inverted index measured 695.7 MB where a single-pass rebuild of the identical rows measured 208.1 MB, and `optimize` + `VACUUM` recovered it in 7.6 s. Whatever the new schema is, **index maintenance has to be part of it** — this is not a schema flaw, it is a missing operation.
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
- **Schema v2 has one mutation authority.** The Python feature entry points listed in
  [`strands.md`](strands.md#sjournalindex) now terminate at
  Rust-owned operations, and `journal indexer` no longer launches an interpreter. Day-ordered
  identities, a typed `day`, a content-type dimension, and `secure-delete` can therefore move together
  without a second production writer preserving the old shape.

## `P-format`

Consistent formatting of **structured journal data** for its consumers — the indexer and the convey apps.

🔴 **No import graph shows the reference implementation's fan-out.** `FORMATTERS` (`think/formatters.py:139-265`) reaches 12 modules — **18 entry-point functions** — by **string key** via `import_module` + `getattr` (`:283-286`), with zero static import edges. It is the de facto read-side inventory of every on-disk shape.

✅ **The indexer half is built.** `core/crates/solstone-core-indexer/src/content/` carries **30 of the reference's 36 patterns** across 15 families, and is the shipped index write path — `think/indexer/native.py` routes every index write operation to `solstone-core indexer` with no fallback. The 30 are exactly the reference's `indexed=True` subset; every family agrees.

⚠ **The six missing are exactly the six the reference marks `indexed=False`:** `entities/*/entity.json`, `*/*/*/audio.jsonl`, `*/*/*/*_audio.jsonl`, `*/*/*/*_transcript.jsonl`, `*/*/*/screen.jsonl`, `*/*/*/*_screen.jsonl`. ⛔ **Name trap:** `content/screen.rs` is the `talents/screen.json` record formatter, **not** the raw `screen.jsonl` one — the file list overstates coverage.

⚠ **The rendered-value half is complete; storage and serving remain open.** `produce_chunks` now carries the full formatter contract: document `header`, chunk `occurrence_time_ms`, and originating `source` record. The index/SQLite layer still stores only content, and the convey read path still cannot serve the added fields for speaker attribution, audio seek, or frame overlays. `S:web:format` has no implementation here at all. ⚠ Rule 1 says the one-to-many end cannot negotiate per-consumer, and an output shape chosen for the indexer is exactly that.

⚠ **Corrections to the 2026-08-05 defect note, measured rather than inherited.** **10 of 36** patterns pin a stream name, not 9 — `*/chat/*/chat.jsonl` was missed because the enumeration scanned the `import.*` family; it is projection-stable, which is why nothing caught it. ⛔ **"Projected names are now being written" was not true** — the projection landed after the last release and no projected stream name has reached a journal. Against the largest journal available, a cutover changes **18 of 538,647** formatted files, all `*_transcript.md` under two import streams, and swaps none. ⛔ **And the failure mode is not a silent `None`:** six of the nine import patterns fall through to a *different* formatter — an AI-chat transcript lands on the audio formatter at `indexed=False`, so it stays formatted and silently stops being searchable. A `None` at least raises.
<!-- historical; chat surface removed 2026-08-20 -->

📌 **The same shape is reachable with no projection involved:** `browser_*_screen.jsonl` matches `*/*/*/*_screen.jsonl` (`indexed=False`) before `*/*/*/browser_*.jsonl` (`indexed=True`), so discovery finds it as one shape and dispatch renders it as another. Latent today.

⚠ **Three matchers, two semantics.** Reference dispatch uses `fnmatch`, where `*` crosses `/`; reference discovery uses `Path.glob`, and this crate uses `glob` with `require_literal_separator`, where it does not. Dispatch is the outlier — which is why discovery and dispatch can disagree about the same file.

✅ **Every family is pinned to a reference-generated corpus** — `core/fixtures/content_families.json`, 40 cases from `scripts/content_family_corpus.py`, resolving each case through the registry by journal-relative path so it pins dispatch as well as render. ⚠ It is a **frozen record**: regenerating it needs a runnable reference tree. A `DIVERGENCES` ledger in `content/mod.rs` makes every difference a written decision; an unrecorded one fails the gate. ✅ **Chat speaker labels are resolved at scan time** from journal config with the reference precedence and a fallback diagnostic, so a rescan preserves the owner's configured label; the ledger has no `Defect` entries.

🔴 **Shape resolution is deliberately two-path, and the path-derived half is PERMANENT — operator-approved 2026-08-06.** ⛔ Do not read the write-new-read-old rule as requiring its removal: this plate is a sanctioned instance, not an unconverted one. The written value wins wherever present; path classification serves content written before the identity existed, and that content is never migrated. ⚠ **Precedence is the load-bearing half** — a written value that does not win is decoration, which is the failure this document already records for `entity_slug()`. 📌 Rendering already takes a **shape** rather than a path (`produce_chunks_by_shape`), so adding the written source is additive and touches no renderer. ⚠ The written value lives in the reserved sidecar `shape.json` beside the content it describes; written wins where present, and path classification is the permanent derived path.

## `P-thinking`

🔴 **A grouping plate.** Holds **two contracts: `generate` and `cogitate`**. Everything connects to it. `P-local`, `P-BYO` and `P-SPP` sit behind it. `resolve_provider()` accepts exactly those two interface names and no others (`models.py:512`).

**`generate` is defined in [`../GENERATE.md`](../GENERATE.md).** Tier **schema + fixture** — an interface format whose closed vocabularies and conformance vectors are pinned as data in `core/fixtures/generate_contract.json`.

🔴 **The plate's import count is not the contract's fan-out, and the difference is tenfold.** 46 production modules import `think.models`; **11 of them import a `generate` entry point** (`generate`, `generate_with_result`, `agenerate`, `agenerate_with_result`), and one of those 11 is the wire itself. The other 35 import model constants, the error classes, `resolve_provider`, or cost helpers — `think.models` is a grab-bag module and its import count is a property of the module, not of this boundary. ⛔ Do not size `generate` work from the module's importers.

⚠ **Ten of the eleven were one-shot; one was a fan-out.** `think/batch.py` was the only caller that needed many completions in flight, and it had three consumers of its own (retired `observe/describe.py`, `apps/timeline/rollup.py`, `apps/timeline/maintenance.py`). That historical asymmetry is why `generate` is one vocabulary in **two framings** rather than one shape or two contracts.

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

🔴 **And the 43 do not cover this plate's own egress failures.** `attestation_not_yet_verified`, `attestation_failed` and `attestation_stale` are the `reason_code` class attributes on the three attestation exceptions (`models.py:275-303`) and are **absent from the taxonomy entirely**, so the blocking predicate answers `false` for all three — while a missing provider key answers `true`. **Operator ruling 2026-08-05: an unverifiable confidential environment holds the owner's material.** The `generate` contract therefore classifies that family `blocking: true` explicitly, and an unknown or absent code resolves to `retryable: false, blocking: true` — the preserving direction. ⛔ The live Python predicate is deliberately **not** changed; the classification lives in the contract as a rebuild invariant. ✅ **The contract now states which field governs** — `GENERATE.md` § *`blocking` governs, and `retryable` is read only when `blocking` is false*. It was published without that, and both fields fire together on every blocking refusal, so two consumers reading in different orders both looked defensible and nothing revealed the disagreement. The premises are pinned by `solstone-core-generate/tests/contract_precedence.rs`: the four transient codes that make the ordering observable, the overlap itself, the preserving default for an unknown code, and this family's override of a taxonomy that omits it.

⚠ **The consequence is unchanged and still ahead of us:** it becomes a behaviour change the first time a converted consumer reads `blocking` off the wire, and whoever lands that inherits a media handler that aborts-and-holds on an attestation outage instead of burning its re-entry bound.

⚠ **Four near-identical entry points, three error semantics.** `generate` / `generate_with_result` / `agenerate` / `agenerate_with_result` each repeat the same nine-step policy sequence; the two `_with_result` forms make schema validation advisory while the two plain forms raise on it. Only `generate_with_result` accepts `num_retries`, `inference_retry_index`, `local_exclusive_admission` and `enforce_responsiveness`. One boundary, four doors, differing on what a schema failure means.

🆕 ✅ **CLOSED 2026-08-11 — the cogitate Python-runtime inventory below is
historical.** Cogitate cutover removed the OpenHands/LiteLLM driver, Python command gate,
prompt assembly, raw-read bindings, and Python contract. The live path is
`cogitate_client.run_cogitate` → `solstone-core cogitate --one-shot`; the
native talent-contract query and dry-run event supply inventory and effective
prompt details. Retain the pre-cut module counts and decisions below as the
conversion record, not as current implementation guidance.

⚠ **The runtime preamble is `cogitate`'s, not `generate`'s.** `COGITATE_RUNTIME_PREAMBLE` is prepended by `providers/cli.assemble_prompt`, reached only from `run_cogitate`; `run_generate` and `run_agenerate` never touch it. It is a **sha256 only** in `core/fixtures/cogitate_contract.json` — 1,989 bytes. ⚠ **And "cross-language" is a location, not yet a fact: zero Rust files read that fixture **— ⛔ FALSE AT HEAD since the native readers: `solstone-core-cogitate` and `-cogitate-tools` both `include_str!` it**** (re-verified 2026-08-09), so today the digest detects only Python-source-versus-fixture drift.

🆕 ⛔ **CORRECTED 2026-08-09 — "not reconstructible" was FALSE, and the correction changes what the fixture is for.** The text *is* recoverable from the repo: `docs/COGITATE.md` carries a **byte-identical** copy — extracted from its fence and hashed, 1,989 bytes, `6614e3fd…`, identical to the constant and to the fixture. 🔴 **But nothing checks that copy.** Grepping every `.py`, `.rs` and `Makefile` for `COGITATE.md` finds one reader, and it reads the doc for an unrelated assertion. So the text is reproducible **by luck of maintenance**, and the doc copy is a live drift hazard rather than a mitigation. ✅ **Disposition: the text goes INTO the fixture beside the digest**, the house style `core/fixtures/local_contract.json` already uses so a divergence *"fails on the string rather than on a hex value that says only 'different'."* 📌 **A digest is the right instrument for one implementation and the wrong one for two** — it can tell the second implementation that it is wrong while being structurally unable to tell it what right is. ⚠ **`COGITATE_DIAGNOSTIC_PREAMBLE` — the second runtime contract, the one the brain readiness probe runs against — is in NEITHER the fixture nor `COGITATE.md`**, and gets the same treatment.

🔒 **AND THE TEXT IS ABOUT TO CHANGE — operator ruling 2026-08-09.** The preamble tells every talent the raw-read tier is *"bounded to the journal root"* and `COGITATE.md` says it *"defaults to broad journal-root read-only."* 🔴 **Both are wrong for half the tools:** `glob` and `grep_search` are inherently recursive and default to `root="."`, which `_broad_recursive_refusal` refuses along with `chronicle/` and `facets/` — **so every default-argument call to either tool refuses.** Measured by execution in both directions: they succeed rooted at `talents/`, at `chronicle/<day>`, or at a single file path; `list_directory` hits the rule only with `recursive=True`. The ruling **keeps the refusal** (the caps bound what comes *back*, not what gets *touched*, and the largest journal measured holds 538,647 formatted files) and **corrects the preamble to state the rule.** ⛔ **Same-day correction, before anything was built: the refusal did NOT need to be made actionable.** `REFUSAL_BROAD_ROOT` already names four repairs — a day `chronicle/YYYYMMDD`, a facet tree `facets/<facet>`, the `entities` tree, or an exact file path. The claim that it returned a bare string came from reading a **truncated console column** as the whole value. ✅ So the refusal ports byte-for-byte and its vectors stay live acceptance criteria; **the contract text is the ONLY deliberate divergence from the reference.** 📌 It is the preamble that sends the model in the wrong direction, *before* it ever calls the tool. ⛔ Declined: relaxing the rule under the existing caps, and porting faithfully with the contract text left wrong. 📌 It is the mechanism behind an already-measured symptom, not a new suspicion.

⚠ **Consequence for anyone reading the fixture: a frozen oracle plus an authorized behaviour change is where a conversion starts drifting silently.** The native crates therefore carry a **divergence ledger** — the pattern `P-format` proved — where each intentional difference from the reference names the vectors it overrides and the ruling that authorized it, and ⛔ an unrecorded divergence fails the gate.

⚠ **Only two provider modules implement `run_generate`** — `providers/local.py` (1,293 lines) and `providers/openhands.py` (2,248). `providers/` totals 21,029 lines; the remainder is install, health and attestation machinery belonging to `P-local` and `P-SPP`, not to this call path.

🔴 **Neither module's line count is this contract's size, and the error runs both ways.** Classified by top-level definition, `openhands.py` is **1,506 lines of `cogitate`** against 742 for `generate` — so two thirds of the larger module belongs to the plate's *other* contract. `local.py` splits 1,144 / 149 the other way. And the call path reaches five more modules the two names hide: `local_endpoint` (551), `local_admission` (386), `fanout_policy` (131), `local_budget` (127) and the provider registry. ⚠ **Sizing `generate` work from either module's total is wrong by about a thousand lines in each direction, and the two errors nearly cancel** — which is how a whole-file total survived being quoted as this boundary's size. 📌 **Measured at the cut: it removed ~9,900 lines and the request that scoped it said ~6,900** — a sum of whole files, four of which survive in whole or in part and one (`think/responsiveness.py`) entirely, because `cogitate` and `P-web` both import it.

✅ **The bundled local arm is already Rust and is already the live path.** `providers/local.py`'s bundled branch delegates to the native `local generate` verb; `solstone-core-local` owns the OpenAI-compatible request builder, schema preparation, response parser, finish-reason normaliser, transport trait and cross-process admission. 🔴 **CONVERTED 2026-08-09 — that list is now empty.** The wire, the dispatch and policy, the endpoint and confidential arms, the cloud arms and attestation are all Rust. `think/generate_wire.py` and `think/schema_prep.py` do not exist; `providers/local.py` fell 1,293 → 350 lines, `providers/openhands.py` 2,248 → 1,760, `models.py` 1,689 → 1,187. `models.generate*` survive as a **thin client** — they delegate to `think/generate_client.py` (442 lines), which encodes a request record and spawns `solstone-core generate`. ⛔ The Python resolves no provider, builds no provider body, classifies no transport error and writes no token log.

⚠ **"No Python writer stands beside them" is true of the `generate` seam and is NOT true of the plate.** `cogitate` is still Python and reaches directly into `models.py` and `providers/shared.py`, so a Python copy of the plate's shared classification and validation stays alive for it — unavoidable until `cogitate` converts, and ⛔ not a leftover to tidy.

### `cogitate` — the plate's other contract, measured 2026-08-09

🔴 **Size it from here, not from a module total.** The surface is **4,513 lines across nine
modules**, plus the cogitate halves of two shared ones. ⛔ **`think/thinking.py` is NOT part of it** —
it is the daily-pipeline orchestrator, and its entire cogitate reach is one import of
`failure_capped` and two call sites. 📌 That is the third time in this document's life that a
whole-file line count has been quoted as a boundary's size.

| lines | module | |
|---:|---|---|
| 1,760 | `providers/openhands.py` | the runtime — LLM build, agent build, the `sol` tool and its executor, the SDK event translator, budgets and monitors, `run_cogitate`. **100% cogitate** post-cut: no `run_generate`/`run_agenerate` remains |
| 823 | `cogitate_read_tools.py` | the raw-read tier: `read_file` · `list_directory` · `glob` · `grep_search`, denylist, caps, traversal and symlink refusal |
| 671 | `providers/cli.py` | `assemble_prompt` (preamble injection), `ThinkingAggregator`, `CLIRunner` |
| 346 | `providers/read_tools.py` | the SDK **binding** for the four read tools |
| 316 | `cogitate_policy.py` | the command gate, budgets, the deterministic-failure vocabulary |
| 240 | `responsiveness.py` | **shared** — the module the `generate` cut preserved entirely for this contract |
| 132 | `cogitate_contract.py` | the two preambles, tiers, capabilities, finalization modes |
| 113 | `providers/emit_final_tool.py` | the `emit_final` finalization tool |
| 112 | `engage.py` | `journal engage <name>`, the owner-facing delegation CLI |

**Six cogitate talents, not nine** — `exec` (`normal`) · `read` and `entity_assist` (tier absent →
`normal`) · `support` (`outbound`) · `partner` and `weekly_reflection` (`synthesis`, weekly,
`emit_final`). The other twenty talents are `generate`. ⚠ `system-read` is claimed by no talent, and
`diagnostic` is not a talent tier at all — it is set by the brain readiness probe.

🔴 **`PROVIDER_REGISTRY` is now a cogitate-only registry**, and `providers/__init__.py` says so in
its own docstring. ✅ **So `openhands-sdk==1.27.*` and `litellm==1.86.1` — hard `pyproject.toml`
runtime dependencies — exist for this contract alone and leave the wheel when it converts.**

🔴 **The Rust `generate` arms cannot carry a tool call, measured not assumed.** All five arms in
`solstone-core-generate-wire` are single-shot: `openai.rs` emits exactly `[system, user]`, and
`tools` / `tool_calls` / `tool_use` appear **zero** times across `anthropic.rs`, `google.rs`,
`endpoint.rs` and `confidential.rs`. ⛔ A multi-turn tool-calling layer is new work, not a
re-wiring.

✅ **The tool surface, by contrast, is already native.** The `sol` tool executor resolves `argv[0]`
next to the interpreter or on `PATH` and **spawns it**, capping stdout/stderr at 6,000 chars with a
30 s timeout — and since the CLI cut, `sol` and `journal` are Rust executables. The whole of "what a
talent can do to the journal" is already on the far side of a process boundary.

⚠ **Two tool-call protocols behind one contract.** `native_tool_calling=True` for the three cloud
providers; **`False` for every local/BYO-endpoint lane**, so those tool calls are prompt-synthesised
by the SDK and parsed by it.

🆕 🔒 **DECIDED 2026-08-10 — the conversion does NOT port the second protocol.** The native runtime
sends a native `tools` array and parses native `tool_calls` on **every** lane, including bundled
local and BYO endpoint. `native_tool_calling=False` is a **compatibility workaround** for a library
that must work against any model; the bundled lane is `llama-server` running `qwen3.5-4b`, both of
which sol pbc ships, and llama.cpp exposes an OpenAI-compatible tools interface. Porting the
synthesis path would mean reimplementing a third-party parser whose format is undocumented as a
contract — the same class of dependency the cut removes.
⛔ **The divergence is only safe if its failure mode is loud**, so it carries three obligations: an
endpoint that rejects a tool-bearing request produces a **named reason code** (⛔ never a silent
fallback), a synthesised tool call arriving as prose under `finish_reason: "stop"` must **not** be
finalized as an answer, and the bundled lane must be **exercised against the real runtime** before
the cut. Full reasoning:
**the native runtime speaks native tool calls on every lane**.
📌 **This paragraph was written before the native conversion and later work were authored without
re-reading it** — the divergence was taken implicitly in landed work and recorded only afterwards.
⛔ A conversion dictionary is only worth what re-reading it costs. ⚠ **And the condenser is local-only** —
`LLMSummarizingCondenser(keep_first=4)`, whose `max_tokens` divides by a `1.125` factor its own
docstring sources to **one production observation** (12,437 served vs 11,237 estimated tokens).
📌 A measurement written down as a constant; carry the provenance, not just the number.

🔴 **A THIRD of `cogitate_policy.py` is dead, and the dead part is the one that reads like the write
guard.** Traced caller-by-caller: `CogitatePolicy` is constructed once and only `classify_command`
is ever called on it; **`check()` has zero production callers** (`_WRITE_TOOLS` and `_READ_TOOLS`
are referenced nowhere else); **`allowed_roots` is stored and never read**; and
**`resolve_read_scope`'s only caller discards its result**, so a talent's `read_scope` has exactly
one live effect anywhere — a prose hint appended to the system instruction. ⛔ **So the
`write_file`/`replace` denial never fires.** The real guarantee behind *"there is no
general-purpose write tool"* is **structural**: the runtime binds `sol`, the four read tools and one
finalization tool, and never registers a write tool. ⚠ A rebuild that ports `check()` faithfully
carries a dead gate; one that *relies* on it relies on something that has never run. ⚠ Of the three
capabilities, only `submit` is enforced in `classify_command` — **`sol` and `reads` are enforced at
tool-registration time**, which is where their native equivalents belong.

✅ **An invariant that is currently true only by the shape of a table, and is worth making
explicit: no access tier holds both `reads` and `submit`.** The only tier that can send anything off
the machine (`outbound`, which `support` uses) has **no raw-read tier at all**. ⚠ That matters
because the credential denylist is seven `fnmatch` globs, so `credentials.json`, `.env.local`,
`api_secret.txt`, `token.txt`, `passwords.md` and `secrets.yaml` are all **readable** while the
preamble says the denylist covers "credentials" — a contract-accuracy gap, ⛔ not an exfiltration
path, and only because of the separation above.

🆕 ✅ **BUILDING — three waves landed 2026-08-09/10, each verified by RUNNING it rather than from a ship report.** `solstone-core-cogitate` holds the contract: both preambles, the access tiers and their capability table, the finalization rule, the live half of the command gate, and the deterministic-failure vocabulary. `solstone-core-cogitate-tools` holds the raw-read tier and, since the tool wave, the `sol` command tool, `emit_final`, `finish`, and **the per-tier binding**. ⛔ **Nothing is wired yet** — the loop and the process boundary are later waves, and no Python has been deleted.

🔴 **THE WRITE-TOOL GUARANTEE IS NOW A MECHANISM, AND IT IS A CLOSED ALLOWLIST.** Every producible binding's tool names must be members of an exact seven-name set, so a newly added tool fails the build until someone widens it deliberately. ⛔ **A denylist here cannot fail on any tree** — the producible names and the forbidden names are disjoint by construction. Proven by mutation: injecting a tool named `journal_write` into every binding reds the check. ⚠ **And state the property accurately** — `sol` *is* write-capable, since `sol call …` verbs persist. The guarantee is not *"no write tool"* but **"no ungated write path": every write goes through `sol`, and every `sol` command goes through the policy gate.**

🔒 **ONE AUTHORIZED DIVERGENCE FROM THE REFERENCE, AND IT IS CARRIED BY A LEDGER.** The runtime preamble moved from **1,989 bytes / `6614e3fd…`** to **2,163 / `39011e2c…`**, correcting the broad-root framing. ✅ The **frozen oracle keeps the reference's value**; the shipped constant and `cogitate_contract.json` carry the new one; a **divergence ledger** in the contract crate reconciles them and refuses **both** an unrecorded difference **and** a stale entry whose case has come back into agreement. ⛔ **The `P-format` precedent implements only the first direction** — it guards `if actual != expected`, so an entry that re-agrees is never inspected. ⚠ **The ledger caught a real defect immediately**: regenerating the oracle re-derived the preamble from *live* source and silently overwrote the divergence's *before* side. 📌 **"Frozen record" was a property of the document, not of the generator.**

⚠ **THE FINISH TOOL WAS AUTHORED, NOT PORTED, AND THAT IS DELIBERATE.** **4 of the 6 cogitate talents finalize through it** — measured — but the tool belongs to the agent SDK this conversion deletes. Its captured description tells the model about *"the **user's** requested task"* and a *"Final message to send to the **user**"*: generic agent-framework copy, and **there is no user on the other end of a talent**. The native description is written in solstone's own terms and pinned by digest.

🔴 **AND THE CONVERSION CANNOT SIMPLY EXTEND THE `generate` ARMS — measured, not assumed.** Two findings a scope review proved against the tree: **`openai` and `google` normalise a valid tool call into a hard refusal** (both map a tool stop to `"stop"`, a tool-call response carries no text, and the shared validation turns `Stop` + blank output into `provider_response_invalid`) — ⚠ unreachable today because `generate` sends no tools, and reachable the moment a tool-calling turn exists. And **`endpoint` and `confidential` cannot represent a tool call at all**: their shared parser **errors** on a `tool_calls` finish reason against a vocabulary frozen in `core/fixtures/local_contract.json`. ⛔ That is a settled one-shot contract, so it gets its own decision rather than being widened in passing.

⚠ **A HAZARD THE LOOP INHERITS AND NOTHING CURRENTLY SOLVES:** the endpoint arm's context fitting is keyed to **one-shot content shapes** and cannot express an assistant turn carrying tool calls or a tool-result message; and the **attested arm has no fitting at all**, because its served window comes from a discovery call it deliberately refuses so it cannot issue an unaudited second request. That leaves a 400-overflow retry — keyed on the serving runtime's **English error text** — as the only backstop. Fine for one shot; **primary for a loop that regrows its prompt every turn**, on a 16,384-token window whose preamble alone is ~2 KB.

🔴 **BUDGETS ESCALATE; THEY DO NOT STOP.** Cost and context share a three-stage ladder — a latched wrap-up warning, then an ultimatum **arming exactly one more turn**, then a hard pause — and turns have their own ladder with a **different instruction at each threshold**. Every message is sent **to the model** and names the finish tool. ⛔ A loop that simply stops at the cap behaves completely differently: the model never gets to wrap up, so runs that would have finished cleanly under pressure end as force-stops with partial results. ⚠ Carry the parts not visible in the strings: each warning is **latched once**; a duplicate action from an **already-counted response id is the same turn** and must be deduped *before* the armed check or an arming turn force-stops itself; the cost fallback deliberately **errs high**; and an **unknown context window must not read as 0%**.

✅ **THE CUT'S VERIFIER EXISTS AND FAILS ON PURPOSE.** `scripts/check_cogitate_cutover.py` was committed while the Python runtime was still in place, reporting **16 findings**, so it cannot be shaped to fit what the cut produces. ⛔ **It does not ask whether a symbol exists** — that reading goes green on a destructive implementation and red on a correct one. It asks whether any Python path still **implements** the runtime: the agent SDK, the policy gate, the read tier, the finalization tool, prompt assembly, the contract text. ⛔ Not wired into a gate until the cut, because wiring it earlier would red the build for every lane.

✅ **The deterministic contract is frozen against execution** — `core/fixtures/cogitate_oracle.json`
(generator `scripts/cogitate_oracle_corpus.py`): **249 vectors, each produced by running the
reference**, each citing the `file:line` that decides it. ⚠ **It is a FROZEN RECORD with the same
clock as the config oracle** — unproducible once the Python tree goes — and is deliberately outside
`make check-core-fixtures`. ⛔ Do not "fix" a vector that disagrees with this document; two
corrections above came from vectors that contradicted the contract *documentation*, and the
documentation was what was wrong.

## `P-local`

Local model runtime, inside the security boundary. **Native**: `solstone-core-brain` owns the durable
record; `solstone-core-local` owns the launch plan, the loopback bind, the connect client, the NVIDIA
probe, the install machinery and `generate`. Those command surfaces are reached as `solstone-core brain
…` and `solstone-core local …` subcommands of the packaged binary. On Linux that binary is static musl
and cannot host a Vulkan loader, so `solstone-core-vulkan-probe` ships separately as a Linux-only glibc
sibling helper.
Python `local_vulkan.py` still owns production probing today; the helper makes the Rust probe shippable
for later selection.

⚠ **Three things here are still Python, each for a stated reason**, so the remainder is not read as
unfinished work:

- the **endpoint and confidential arms** of the provider module, and the request builder, schema
  preparation, response parser and finish-reason normaliser they share with the bundled arm — those
  belong to `P-BYO` and `P-SPP`, and they are **egress**;
- the **install-state, install-lease and fit-report** machinery — shared with the speech-recognition
  sidecar and the vision detector. Their records are **per-provider on disk**, which is the only reason
  the local half could convert on its own;
- the **tool-using runtime's** local branch, which is `cogitate`'s contract and not `generate`'s.

⚠ `providers/oci_image.py` has **no runtime importer at all** and is reached only from
`scripts/repack_cuda_runtime.py`. It is packaging-time machinery, live but outside the shipped runtime
path, and a rebuild does not carry it into the runtime.

⚠ The boundary is loopback HTTP **plus a durable record.** Calling it a types boundary drops the durable
half.

🔴 **The durable record is NOT this plate's.** `providers/brain_state.py`'s own docstring says *"the
single selected thinking lane"*, and `BrainLaneId` is a five-member closed set — `none`, `bundled`,
`spp`, `byo-cloud`, `byo-endpoint`. `build_active_brain_fingerprint` has a dedicated branch for each: a
hashed cloud credential, an endpoint plus served model plus hashed credential, those **plus a
confidential provenance digest**, and the bundled runtime's own digest. One file, one writer, five lanes
— two of which are the egress siblings. ✅ **So it converts once, for all five**, with the egress lanes
continuing to run their own probes and handing the *outcome* to the single writer. Converting it "for
the local lane only" would put a native writer and a Python writer on one file, which is the shape the
cutover rule forbids and the case where it would be silent.

⚠ **"three lanes" means the three WRITE lanes** — refresh, prerequisite renewal, runtime-failure marker
— not the five lane **ids**. Both readings are live in the module and they are different dimensions.

**Durable artifacts, measured rather than listed from the module that names a path:**
`health/brain.json` · `health/brain-fingerprint.key` · `health/brain-refresh.lease` ·
`health/brain.json.lock` and `health/brain-fingerprint.key.lock` (stable sidecars, `LOCK_EX|LOCK_NB`,
⛔ never unlinked) · `health/providers/local.json` · `health/providers/runtime/` · `health/local.ctx` ·
`health/local.port` · `health/local-inference/YYYYMMDD.jsonl` (one row per inference, success **and**
every error path) · `health/local-inference-admission/` (the slot ticket queue).
📌 **The last four were missed by an inventory that read the modules naming paths** — the lock sidecars
are built by the locking primitive and the inference log by the admission module. **Count the writers,
not the filenames.**

🔴 **Atomic replace is not atomic read-modify-write.** Every write to the record takes a lock on the
stable sidecar *first*. An implementation that does the atomic replace alone converts a loud conflict
into a **silent lost update**, and it passes any criterion that says only "writes are atomic".

🔴 **An OS lease cannot be held by a one-record command.** The refresh permit, the prerequisite-renewal
permit and the install lease are all `flock` on an open descriptor, which the kernel drops when the
holder exits — so a `begin` that runs as its own process has released the permit before its caller uses
it. ✅ The shape that fits is the session child `GENERATE.md` already ruled for the same question: a
terminal record then EOF means finish, a **bare EOF means the caller is gone** and the work is abandoned.
⚠ And process death is only half — a caller that *hangs* never dies, so the child needs its own bound or
the lane reports busy forever with nothing red.

🔴 **The canonical fingerprint diverges from `serde_json` in THREE ways, not one**, and none of them
fails loudly — the digest simply stops matching, the record's evidence is fenced out, and the journal
quietly re-enters `checking`. `ensure_ascii` escapes non-ASCII; a non-BMP codepoint escapes as a UTF-16
**surrogate pair**; and exponents are written `1e+22` and `1e-07` rather than `1e22` and `1e-7`.
✅ All three, plus the rules that *do* port, are pinned in `core/fixtures/local_contract.json` with the
canonical text recorded beside each digest, so a divergence fails on the string rather than on a hex
value that says only "different".

⚠ **Do not carry** the in-loop USD ceiling — it fabricates a cloud price for local runs and force-stops
them. Keep the **context**-fraction half of the same function. ⚠ It sits on `run_cogitate`, not
`run_generate`, so nothing in a `generate` rebuild reproduces it; it is recorded here so whoever converts
the local tool-using loop inherits the ruling rather than the code.

⚠ **Do not carry the loopback "guard" as written.** The `cmd` list is built with `"--host", "127.0.0.1"`
hardcoded and a later statement is `if "0.0.0.0" in cmd: raise` — a membership test against a literal set
three to twenty lines above. It cannot fire at runtime and would miss `--host=0.0.0.0`, `::`, or a
hostname. ⚠ **There are three copies in `think/supervisor.py`, two of them this plate's and one the
speech-recognition sidecar's.** ✅ **The honest statement of the same invariant is
`think/services/spp_transport.py:189-195`** — it binds `("127.0.0.1", 0)` and writes down why same-UID
reachability is acceptable and which constraints are actually load-bearing: loopback bind, ephemeral
port, per-session lifetime, creation only after the environment is verified, teardown on failure. Carry
that one. ✅ **And carry it as a type rather than a predicate** — build the argv *from* a bind value that
cannot express a non-loopback address, so there is nothing left to check.

🔴 **One probe cannot move into the packaged binary, and it is linkage rather than effort.** The Vulkan
device enumeration calls `libvulkan` in-process through `ctypes`, inside a subprocess of itself so a
driver fault cannot take down the journal; a second, separate call reports per-device memory in use.
`solstone-core` ships **static musl** on both Linux lanes and a static musl binary cannot `dlopen`.
✅ The shape it needs is already shipped here for exactly this reason — a separately packaged,
dynamically linked helper on its own glibc lanes, the way the speaker analyzer ships. ✅ The NVIDIA probe
is **not** affected: it is a subprocess and a file read.

The shipped shape is `solstone-core-vulkan-probe`: a Linux-only sibling helper
which dynamically loads the host Vulkan loader and emits the isolated JSON
device protocol. It must remain independent of the ONNX speaker helper so an
audio-runtime loader failure cannot prevent GPU enumeration.

🔴 **A native verb reaches this plate as `solstone-core <verb>`, never as a standalone binary.** The
wheel check builds an **exact** member set from a one-name script list, and the Python↔Rust seam resolves
that one name behind a version handshake. A separate executable is unreachable on an installed host, and
a Rust-only gate cannot see that it is.

## `P-BYO`

An owner's own token to a known provider, or their own OpenAI-compatible URL and token. ⛔ **Egress.** Will split per provider.

## `P-SPP`

Confidential hosted processing. ⛔ **Egress.** Attested, non-retained. ⚠ **Fails closed by the absence of a fallback branch** — a refactor that tidies it can open a downgrade path with no test going red. Make "no downgrade path" an explicitly tested invariant.

## `P-speaker-id`

Per-statement embeddings → speaker fingerprints. ✅ A native kernel already sits behind an argv+stdio wire with an algorithm-identity handshake.

⚠ `.npz` sidecar and `speaker_labels.json` are Python-only. ⚠ The voiceprint corpus is real-person biometric data. 🔴 **Years of voiceprints must survive with no re-teach** — that is a shipped promise.

🔴 **A HALF CUT HAS ALREADY LANDED HERE — `core/crates/solstone-core-entity/src/store/voiceprints.rs` is a complete native reader AND writer of `entities/{id}/voiceprints.npz`** (NPY parse and write, batch append, metadata rewrite, remove-by-key, Python-equality-compatible key canonicalisation). **Only its merge half is reached** (`store/merge.rs:894-960`); `save_voiceprints_batch`, `remove_voiceprints_by_key`, `rewrite_voiceprint_metadata`, `load_entity_voiceprints_file` and `load_existing_voiceprint_keys` have **zero callers anywhere in the repo** outside their own re-export chain. Meanwhile the Python writers are live (`apps/speakers/attribution.py:1762`, `discovery.py:2109`, `:2859`). ⛔ **So this file has two writers in two languages today.** A conversion here **calls the existing store API and widens its export where needed** — ⛔ it does not add a third implementation, and the file stays `P-entity`'s hold.

🔴 **Rule 4a: the identity is minted at write time and DELIBERATELY DISCARDED — it is not merely "recomputed".** `observe/transcribe/utils.py:83,88` mints a 1-based ordinal into `stmt["id"]`; `observe/transcribe/main.py:625-635` (`_statements_to_jsonl`) builds each record as `{start, text, [source], [speaker]}` and **never writes it**. Six sites then re-derive it from file position — `apps/speakers/attribution.py:161`, `:944`, `bootstrap.py:762`, `routes.py:1019`, `edges.py:162`, and `solstone-core-indexer/src/edges/speaker.rs::load_transcript_texts` — and two durable stores persist it as a join key. ⛔ **Persisting it costs nothing: the value already exists at the write site.**

✅ **The resolver landed 2026-08-08** — one place resolves a `sentence_id`: a persisted integer wins, absent falls back to the 1-based ordinal after the header, and a non-integer or non-positive value is ignored *and counted*. ⛔ **There is deliberately NO upper bound.** A first implementation bounded it on the highest value seen on a reference journal and thereby discarded the identity of every row at or above that sample — ⚠ **an observed maximum is an observation, not a contract**, and a grounding document that states a range must say which it is.

✅ **BUILT 2026-08-08 — the reader, the writer, the label store and the attribution layers are native.** `solstone-core-speaker-id` now holds the transcript reader and `sentence_id` resolver, the analysis-output writer (`<stem>.jsonl` rows carrying `sentence_id` + the `<stem>.npz` sidecar), the label and correction stores, and attribution layers 1-3, behind the `solstone-core speaker-transcript-write` subcommand. Each verified by running it, not from a ship report. 🔴 **The `Person` guard is an EXACT allowlist** (`t == Some("Person")` — refuses `Human`, `person`, absent) applied at **all three** admission points including **Layer 3's empty-candidates fallback**, which is the highest-volume contamination path; ✅ **the setting channel is KEPT and gated**, ⛔ not removed — removing it empties `candidate_entities` on exactly the damaged segments and widens the fallback to every non-principal entity. ⛔ **Still Python:** voiceprint accumulation, the owner centroid, discovery/bootstrap, and the identify/undo ledger.

✅ **SHIPPED 2026-08-09 — the transcribe cutover is native and verified by running it, not from a ship report.** `solstone/observe/transcribe/native.py` routes the handler's main-success, terminal-empty, and decode-failure JSONL paths through `solstone-core speaker-transcript-write`; `_statements_to_jsonl`, `_write_empty_processing_jsonl`, and `_write_failed_processing_jsonl` are deleted. Direct execution against the compiled binary confirmed the response contract, the `destination-exists` redo guard, and orphan-`.npz` auto-heal.

🔴 **A CYCLE THE CONVERSION CREATES, AND THE CONSTRAINT IT LEAVES FOREVER.** `indexer -> speaker-id -> entity -> indexer-store -> indexer` closes as soon as the indexer reads transcripts through the native reader, because the writer needs entity's NPY primitive. ⛔ **Neither edge is individually wrong.** The resolution is a zero-dependency `solstone-core-npy` leaf plus a crate split — a format/IO leaf with the entity-dependent policy above it — ⛔ **not** widening `solstone-core-journal-io`, which `core/deny.toml` gates behind a per-crate authority allowlist. 🔒 **Permanently after that: `speaker-id` sits inside `entity`'s dependency closure, so any type shared between the label store and the voiceprint store must live in `solstone-core-npy`, `journal-io` or lower.** ⚠ A future "just one struct from entity" re-closes it, and **it only appears when a branch is combined with main** — never in the branch alone. ⚠ Two NPY readers now exist and they **disagree** on 0-d shapes and payload-length checking; merging them changes a reader tolerance sitting under 51,186 voiceprint rows.

🔴 **THOSE SIX SITES USE THREE DIFFERENT LINE-SPLITTING PRIMITIVES AND THEY DO NOT ALL AGREE.** Python `readlines()` (universal newlines) · `str.splitlines()` (Unicode line boundaries) · Rust `str::lines()` (`\n` only). Executed over an adversarial corpus, three cases diverge: a **lone `\r`** line ending gives Python two sentences and Rust **zero**; a **raw U+2028 or U+0085** in a text value makes `edges.py` drop sentences 1–2 and renumber the third line to 3. ⛔ **The second class is armed BY the conversion** — `json.dumps` defaults to `ensure_ascii=True` and `serde_json::to_string` does not escape non-ASCII. **The native writer must escape non-ASCII in transcript rows.** ✅ Agreed by all three: a blank or unparseable line **consumes** an ordinal, so the legacy positional fallback must count them.

✅ **The no-re-teach promise is keepable at the read layer, proven by execution against a real journal (2026-08-07):** the shipped native reader parsed **44/44 voiceprint files and 51,186/51,186 rows in place** — 0 unreadable, 0 width mismatches, 0 metadata parse failures, 51,186 with an integer `sentence_id`, 0 zero-vector embeddings. ⛔ The probe binary travelled to the data; the biometric data did not travel. ⚠ It proves the *read* half only — not writer round-trip, not resolution equivalence, and N=1 journal.

🔴 **30.2% OF THAT STORE IS NOT THE OWNER'S TEACHING — IT IS A DEFECT'S OUTPUT, AND IT CHANGES HOW THE PROMISE READS.** Aggregating voiceprint rows by entity `type` on a reference journal: **`Person` 35,728 rows across 34 entities**, against **15,458 rows across 10 NON-`Person` entities** (`Tool` 12,509 — one entity alone holding 11,949 — `Project` 2,412, `Company` 537). That is the `structural_setting` defect: the conversation's *setting* (`office`, `conference`, `restaurant`, `outdoors`) fed into the speaker-candidate channels and written at **`confidence: "high"`**. ⛔ **So *"years of voiceprints must survive with no re-teach"* is a promise about the 35,728 rows of real teaching, NOT about the 51,186 total — "no re-teach" does NOT mean "preserve everything."** ⚠ **And the ordering is fixed, not a choice: the `Person` guard must land in the rebuild BEFORE any cleanup**, because deleting the contaminated entities first is undone by the next attribution pass; cleanup is then a separate, owner-visible act, ⛔ never a silent migration inside a conversion wave. 📌 A read-layer proof that reports "51,186/51,186 rows still read" is the right number for *the reader still reads everything* and the **wrong** number for *the owner's teaching survived* — state both.

**The store's real contract, measured, not inferred** — one metadata keyset on all 51,186 rows with no variation: `added_at`(int) · `day`(str) · `last_seen_ts`(int) · `segment_key`(str) · `sentence_id`(int) · `source`(str) · `stream`(str); embeddings `<f4` (N,256); members exactly `embeddings.npy` + `metadata.npy`. ✅ The 4-field key is **unique within an entity** (0 collisions), which is the scope `remove_voiceprints_by_key` operates in. ⚠ 8 keys are shared **across** entities (16 rows) — measured, ⛔ not diagnosed.

✅ **THE ENVELOPE LANDED 2026-08-08.** The durable format used to carry **no version and no encoder identity** while the native reader hard-coded width 256, so 🔴 **an encoder change made every existing voiceprint unreadable with no migration path — a forced re-teach.** It now carries an **additive** `envelope.npy` (format, version, encoder id + sha256, width); the reader **tolerates** unknown members and a newer version rather than erroring, and **refuses the mutation** instead — *open on read, closed on write*, the store's house style. **Absence is legacy, never corrupt.** 🔴 **The encoder identity is CALLER-SUPPLIED and the store never invents it** — `ENCODER` is private to `solstone-core-speakers-analyze`, which is host-excluded and not a dependency of the entity crate, so a local copy would be the very defect the envelope fixes. `merge_voiceprints` preflights and refuses across an encoder boundary. ✅ Verified by re-running the shipped reader **in place** on a real store after the change: **44/44 files, 51,186/51,186 rows, all read as version 0, zero unrecognized members.** ⚠ **"The store self-heals to v1 on first mutation" is FALSE until the Python writers are deleted** — all three transforms in `think/entities/voiceprints.py` return exactly `{embeddings, metadata}`, so every Python write strips the envelope back off. Correctness is unaffected (v0 reads fine); ⛔ no report may read "v1 present" as "always stamped."

⚠ **`merge_voiceprints` writes this file with `atomic_replace` and NO `hold_lock`**, while every other native entry point takes one and Python's `update_npz` takes one on every write — so a native merge concurrent with a Python append is a **silent lost update**. ⛔ Measured by reading both call paths, ⛔ **not** observed to have fired. `P-entity`'s to answer.

🔒 **`talents/speaker_labels.json` keeps its exact path and filename.** `think/retention.py:209-241` reads the `.npz`↔labels pairing as the *segment incomplete* predicate gating raw-media purge — **12 segments on the reference journal are held by it today**. ⚠ **The speaker wipe defeats that gate**: it deletes the `.npz` *and* both label paths, so after a wipe the check cannot fire and previously-held segments become releasable. ⚠ **Updated 2026-08-09 — this used to cite `apps/speakers/wipe.py:60-65`, which NO LONGER EXISTS.** The cut converted it to `solstone-core-speaker-resolve/src/artifact_wipe.rs`, which removes `speaker_labels.json` (`:77`), `speaker_corrections.json` (`:83`), `voiceprints.npz` (`:92`), `owner_centroid.npz` (`:98`) and `owner_candidate.npz` (`:105`) **in the same categories together** — ✅ **the defect was ported faithfully, which is the right call for a conversion**, and the fix is now a Rust change rather than a forbidden Python one. **`P-journal-retention`'s to answer, not this plate's.**

✅ **THE CUT LANDED 2026-08-09 — this plate is DONE. Speaker identity runs entirely in Rust, and an owner's existing voiceprints resolve with no re-teach.** The per-statement embeddings, the voiceprint store, the label and correction stores, attribution layers 1-3, accumulation, the owner centroid, discovery, identify/undo and the backfill ledger are all native; every Python durable writer of the seven paths is retired, `apps/speakers/speaker_resolve_transport.py` is the sole Python transport, and the native verbs answer on the **built** binary — including the **label-write and correction verbs, which did not exist before the cut**. 🔴 **Proven by re-running the shipped reader IN PLACE on a real store a fourth time: 44/44 files, 51,186/51,186 rows, all version 0, zero unrecognized members — identical to all three earlier runs.** ⛔ The probe travelled to the data; the data never travelled.

🔴 **"ZERO PYTHON" DOES NOT MEAN THE MODULES ARE GONE, AND A GATE KEYED ON THAT IS WRONG.** `save_speaker_labels` and `append_speaker_correction` still **exist** — as **transport shims** calling `native_speakers.*`. **The name survives; the write does not.** Several durable-write functions also survive **for the entity-merge flow only** (`update_speaker_labels`, `remap_speaker_corrections_for_entity_merge`, `apply_entity_merge_voiceprint_inverse`, `save_voiceprints_batch`), reachable **solely** from `think/entities/merge.py` — verified caller by caller. ⛔ **So the enforceable question is never "is the symbol defined"** — that reading goes green on a destructive implementation *and* red on a correct one — **but "does a call reach a write primitive with a path resolving to a speaker artifact."** `scripts/check_speaker_identity_cutover.py` asserts exactly that against a committed census. ⚠ **Its unit tests run `all_files=True` while the Makefile runs the CLI's `git ls-files` default — different code paths, so a green unit test does not prove the wired gate catches anything.** Falsify the mode you actually ship.

⚠ **ONE THING THE CUT DELIBERATELY DID NOT CLOSE.** ✅ **The owner-contamination guard is now native and fail-closed for both automatic and UI writes; the Python `routes.py` surface no longer exists.** 🔴 **There is still no speaker-side cross-language check.** The former dedicated Python-oracle Makefile rail and its last audio-decode leg have been retired, and the speaker crates never declared a matching feature. ⛔ **That rail was never evidence about this plate, and it no longer exists to be misread.** The Python oracle stays recoverable in history at `45990f652`.

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

The speculative-facet aggregation and candidate-record upsert are native, and `journal facet-candidates` dispatches to the sibling `solstone-core` binary, proven under the sibling-interpreter poison.

## `P-journal-config`

`journal/config/journal.json`. Durable, `0o600`, mutated under `hold_lock` + `atomic_replace` with an explicit transaction type.

⚠ **The PLATE is that file; the owner VERB reaches wider — do not conflate them.** `journal config`
(native since 2026-08-13, `solstone-core config`; `think/config_cli.py` deleted) touches this file only
through `journal_is_active`. Its other surfaces are **not** this plate's: `~/.config/solstone/config.toml`,
and the managed `sol`/`journal` wrapper scripts in `~/.local/bin/` — whose embedded `SOLSTONE_JOURNAL`
and `SOL_BIN` it parses and rewrites under an `flock` on `~/.local/bin/.sol.lock`, with the version
marker **read `[1-7]` and written `7`** — and **the owner's journal directory itself**, which `--move`
renames. ⚠ `--merge` is a **stub** that refuses with fixed owner copy. ⚠ Eight destructive-path exits,
seven of them `2` and one `1` after a post-rename rollback that prints nothing on success.

🔴 **The single-owner lint is now blind, exactly as this section predicted.** It said: *"once the
durable write lives behind a process boundary, the real writer is a subprocess, which no lint over
Python call shapes can observe."* That boundary now exists. ⛔ Do not read a green
`check_journal_config_owner.py` as the invariant it used to approximate.

⚠ **"Read by 30 production modules" is one row of four, and it is a floor for the plate.** Measured by AST over the non-test tree: **31** modules import the reader (30 excluding the config module itself, so the figure is right for what it counts), **19** modules make **46** calls to the mutator, 17 import the read-side helpers, 7 import the error type. **The union is 55 production modules**, and the writer half is where a cutover's work is. ⚠ None of the 36 reader call sites sits inside a loop.

✅ **Carry forward — this is the house style, not a local quirk.** `CorruptConfigError` (`think/utils.py:53-68`): a **missing** config returns deep-copied defaults; a config that **exists and will not parse raises**, in owner voice — *"I couldn't read your settings file… Your settings were NOT changed."* Two deliberately different postures on two failure modes, never silently substituting on the dangerous one.

✅ **The style now holds on both sides, and restoring it was the plate's real work.** It was broken in
four readers when this plate was measured — two Rust, two Python — and each broke it the same way:
by answering as though the file were *absent* when it was present and unreadable.

| Reader | What it used to answer on a config that would not parse |
|---|---|
| the edge indexer's owner-timezone read | `Tz::UTC` — and the owner timezone buckets a segment into a **day**, so records filed under the wrong date with no signal |
| the chat-label caller | substitute speaker labels, **erasing the owner's own name from indexed chat** — the same harm `P-format` had just closed on a different path. ⚠ Its helper distinguished missing from malformed *and said so in a doc comment*; only the caller threw the answer away | <!-- historical; chat surface removed 2026-08-20 -->
| `journal_is_active()` | `False`, so an onboarded journal presented as un-onboarded and the owner was sent to the first-run wizard |
| `doctor`'s two STT checks | the *default* backend, and *"not applicable"* — the diagnostic tool answering about a file it could not read |

📌 **Three lessons worth more than the fixes.** **A doc comment is a claim, not a measurement.**
**A read path with no home does not stay unread — it grows private copies, and they diverge.** And
**presence is decided by the read attempt, never by `exists()`**, which answers `false` for *"I cannot
tell"*: a symlink loop or an unreadable parent reads as *no config at all*, and every reader then
substitutes defaults for settings that are sitting right there.

⚠ **The owner-visible path, traced:** the convey root gate reads `journal_is_active()` → `False` → redirects to the **first-run wizard** → the wizard materializes → raises → 500 whose JSON `detail` carries the sentence, rendered as raw JSON in a browser. So the owner is told their journal is not set up and then shown a JSON error. The fail-closed **writer** is the only reason their settings survive it.

🔴 **There are TWO default sets and conflating them is a defect no test of either alone catches.** A **reader** that finds no config yields `journal_default.json` verbatim — identity fields empty. A **mutation** that materializes a config starts from those defaults **plus** the OS user record and `/etc/localtime`. ⛔ A reader handing back the materialized set, or a mutation starting from the plain set, is wrong; the second silently drops the owner's name from every journal created through that path. ⚠ A Rust materializer once supplied a **three-key** default set where the real one has **eleven** sections; because an existing config is authoritative and never merged with defaults, a config materialized through it would have been permanently missing eight of them. ✅ Closed by making the defaults **non-negotiable** — no caller supplies a default set, and the two adapters are named so neither can be reached by accident.

✅ **A rebuild reads what Python could have written — measured, both shapes.** `json.dumps` **emits** bare `NaN` and `Infinity` and accepts them back; `serde_json` **hard-rejects** all three non-finite tokens, so a config the reference writer could have produced is unreadable by its replacement. ⚠ Reachability was measured too: no config writer coerces a float and nothing in the production tree can produce one, and the reader's answer is a strict-load failure that leaves the file untouched — the right posture. Integers, by contrast, are **not** rejected: `u64::MAX` round-trips byte-identically, and only beyond it does a value degrade to `f64`. 📌 That refuted a line of [`../PORTING.md`](../PORTING.md) § Data Boundaries, now corrected there. **The general rule: measure reachability rather than assuming it, and do it while the reference still runs.**

⚠ **The config file being the source of truth is an external commitment made in writing.** A contract-breaking pass here can violate it by accident.

⚠ **The single-owner lint is real, unreachable, and holed.** `scripts/check_journal_config_owner.py` enforces one transactional owner, but it runs only from `install-checks`, which `ci` no longer reaches — and it detects replacement only through `atomic_replace` / `os.replace` / `Path.replace` / a second `hold_lock`. ✅ Both are closed: the detection set now covers `write_text` / `write_bytes`, `open()` in a writing mode and `Path.open()` in a writing mode, and the tree carries no violator. ⚠ **Proved two-directionally** — it fires on one planted violation of each class, stays silent on a reader opening the same path for reading, and passes on the real tree. ⛔ A gate asserted rather than exercised is how this one sat blind.

🔴 **And it is about to see less, not more.** Once the durable write lives behind a process boundary, the real writer is a **subprocess**, which no lint over Python call shapes can observe: any module could invoke the config verb and the lint would still report `pass`. ⛔ Do not read a green single-owner gate as the invariant it used to approximate.

## `P-journal-retention`

The logic that decides what raw media is retained, and what logs are retained for how long. Policy and marking live in `core/crates/solstone-core-retention/` (`policy.rs`, `marks.rs`); the settings-web HTTP handler and `retention_executor.rs` drive that surface. A proposal is not a removal — the owner approves marked identities before anything is released.

🆕 🔴 **Widened 2026-08-05 by operator ruling: this plate EXECUTES every removal of owner media, and it is the only plate that does.** Other plates **request**; retention removes. Three consequences that are not local to retention:

1. ⛔ **The segment is the unit of deletion.** A segment is removed whole — every file, leaving a `tombstone.json` — or it is not removed. **There is no partial-segment delete.** The *mixed* disposition (keep vs drop part of a segment) existed only to serve a capability the product no longer offers; the classification itself survives as the receipt's cost disclosure.
2. ⛔ **`transcribe` stops unlinking VAD-empty raw audio.** It writes the terminal-empty marker exactly as it does today and hands the raw to retention. One subsystem, one policy, one place to look when owner media went.
3. 🔴 **Retention notifies `P-index` of the paths it actually removed, after removing them.** ⛔ Ordering is the contract: the index is told about removals that have happened, never about removals that are intended. An index prune is not a removal — the index is rebuildable by design, so pruning it is a cache invalidation and a rebuild undoes it. **Anything an owner is told was removed must be removed from the chronicle first.**

🆕 ⛔ **CLOSED 2026-08-05 by operator ruling, by removing the question rather than answering it. Do not re-derive it.** This entry used to record an open call: legacy segments holding one source's data beside another's, where an owner deleting one source either loses the segment whole or keeps the data they asked to remove.

🔴 **The source-delete affordance resolves a source name to a SET of whole segments and hands that set to the door.** There is still no partial owner-directed delete of any kind. The legacy-mixed problem was a *disposition* (keep vs drop part of a segment); that disposition stays gone. `DELETE /app/devices/source/{source}` names a source (`location`); selection lives with that surface.

✅ **Two selection surfaces exist** — the per-segment delete route under the transcripts app, with containment via `commonpath` and a **10-second undo window**, and the source-delete route which expands a source name to a set. ⛔ Retention never resolves anything; it receives owner-chosen targets. Selection lives with the surface.

🔴 **The whole-segment verb therefore takes a SET.** One receipt covers it, and per-target failures are receipt rows. ⛔ It is not all-or-nothing: an owner deleting forty segments must not lose the thirty-nine that succeeded because the fortieth was unreadable.

⛔ **Retired with it:** the *partial* source-delete implementation and both of its branches · the location-only vs mixed *disposition*. The owner-facing source-delete **route** remains. A mixed classifier returns **only** as a receipt counter (cost disclosure) and never selects a disposition. The reserved-name set feeds that disclosure and the ingest upload guard, not a partial delete.

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

✅ **Resolved in the completed raw-media migration — historical finding:** **The raw-media policy has no runner.** `purge()` had no schedule entry, no maintenance routine and no timer; its only two callers were a CLI command and one HTTP route. So `retention.raw_media: "days"` and `"processed"` were owner-settable, rendered in the owner UI, and **never executed**. ⚠ The two doors also carried **opposite destructive defaults** — `--dry-run` defaulted *false* on the CLI, *true* on the route — and the route required `older_than_days >= 1`, so it could not express the configured policy at all. `purge()` is now removed; both doors call `mark()` against the configured policy and expose neither override nor dry-run mode.

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

⚠ **Historical at ruling; partly superseded in the completed raw-media migration:** **Two logs this plate writes and never prunes**: `health/retention.log` and `health/pruning-runs/{day}.jsonl`. The log-retention class list covers `chronicle/{day}/health/`, not the journal root's `health/`, so the subsystem that prunes logs does not prune its own. `health/retention.log` no longer has a writer; `health/pruning-runs/{day}.jsonl` still grows from `raw_media_offload` audit records written by offload.

⚠ **Historical at ruling; partly superseded in the completed raw-media migration:** **Two of this plate's writes bypass the journal write discipline** and pass the access lint by not importing the primitives it inspects: `_write_retention_log` has been deleted; `write_prune_audit` still uses a bare `open(path, "a")` (`think/pruning_audit.py:55-56`).

⚠ **Two config keys are dead.** `retention.storage_warning_disk_percent` and `retention.storage_warning_raw_media_gb` are read (`retention.py:413`, `:439`) and written nowhere — no route, CLI, migration or UI. The owner cannot change either.

⚠ **`retention.raw_media` accepts an arbitrary string.** The init-finalize route validates the day count and never checks the mode for membership, and `RetentionPolicy.is_eligible` (`:273-281`) reads an unknown mode as *keep* by falling off the end — it fails closed **by accident**, not by construction.

## `P-web` — SPLITS into `P-web-[app]`

✅ **TEN plates are approved.** The remainder are deferred, not dropped.

| Plate | From | Note |
|---|---|---|
| `P-web-devices` | the app labelled *devices* | device metadata, posture, adding/managing devices; takes the dashboard. ⚠ **The observer LAYER dies inside it — the UI does not** |
| `P-web-network` | `network` | VPN-method config and network status **only** |
| `P-web-home` | `home` | hosts the **retention approval surface** — when retention marks something, it populates here |
| `P-web-speakers` | `speakers` | 🔵 **FIRST `P-web-*` LANE, BUILDING 2026-08-10.** ⚠ **Distinct from `P-speaker-id`** — same word, different plate: this is the web surface, that is the engine. 🔴 **Measured, and the plate-size framing was misleading in three directions:** the directory is 34,635 Python lines, **61% of which are tests** a Rust rebuild replaces rather than ports, and **3,536 production lines are the INGEST pipeline's** — `attribution` · `candidate_tracker` · `edges` · `encoder_config` · `evidence`, reached from `observe/transcribe` and `think/`, living here by directory accident. The web surface proper is **~10,000 lines**. ⛔ **Deleting those five with the plate breaks the sense pipeline.** 📌 **Size a `P-web-*` plate by its CONSUMERS, never by its directory** — one import sweep moved the real size by 26%. ⛔ **Retire *"substantial dead code"*: there is essentially none** — of 389 top-level symbols, 4 are test-only and 0 unreferenced; the surface falls because tests are replaced and engine code is reclassified. ⚠ A first instrument reported 46 dead symbols and every one was a decorator-dispatched route handler |
| `P-web-thinking` | `thinking` | |
| `P-web-body` | `body` | ⛔ owner **body** data — never "health" |
| `P-web-entities` | `entities` | |
| `P-web-settings` | `settings` | |
| `P-web-transcripts` | `transcripts` | Reference renders owner-local times; is_supervisor_up() is a seedable branch rather than an irreducible host artifact; chat-state reads synthesize sol_message_origins with stringified list-index keys and are not the persisted chat log; this is a deliberate, pinned reproduction of the existing read-only on-disk shape rather than a new writer-derived naming violation. | <!-- historical; chat surface removed 2026-08-20 -->
| `P-web-import` | `import` | |

⏸ **Deferred, to be grouped rather than plated one-for-one** — decided when the work gets closer: `support` · `backup` · `timeline` · `sol` · `health` · `chat` · `search` · `tokens` · `activities` · `news` · `reflections` · `stats` · `facets`. ⚠ Several are smaller than their own handoff would be.
<!-- chat's deferral is resolved by deletion, not conversion -->

**Measured facts about the deferred apps and awareness, 2026-08-13** — engineering findings, not a grouping decision:

- 🔴 **`activities` the app and `activities` the internal facet model are different things, and the app's HTTP surface is NOT all UI.** Six routes — `GET`/`POST /app/activities/api/day/{day}/records`, `GET /app/activities/api/day/{day}/record/{span_id}`, and `POST .../{span_id}/{update,mute,unmute}` — are the server for the native `activities` CLI grammar, declared in that app's `native/authority.toml`. ✅ **Measured: the app's own web client fetches NONE of them**, only `/api/day/{day}/activities` and `/api/activity_output/{path}`. **The UI set and the CLI set do not overlap at a single route.** ⛔ Removing the web UI must not remove the six.
- 🔴 **`solstone/apps/{activities,awareness}/native/command.rs` are `#[path]`-compiled into `solstone-core-sol-client`**, and all four files under those `native/` directories are pinned by `scripts/check_wheel_contents.py`. ⛔ Deleting either app directory wholesale yields a tree that does not build — and a binary that does not build can go neither green nor red.
- ⚠ **`awareness` is API-only**: no `workspace.html`, no `app.json`, no registry row, and `GET /app/awareness/` is deliberately a 404. Its five native routes serve the CLI, so it is not an owner-visible surface in the reserved sense.
- ⚠ **`reflections` the APP and `reflections` the CONTENT FAMILY are separable.** `reflections/weekly/*.md` is produced by the thinking pipeline, registered as a content family, and indexed; none of that is the app. ⚠ Two things outside the app link to `/app/reflections/{day}`: the home surface's latest-weekly card, and **convey core's chat source attribution** — the second is shell code, not an app.
<!-- historical; chat surface removed 2026-08-20 -->
- ⚠ **A `maint/` directory inside an app is not necessarily app machinery.** `activities`' one-shot icon migration operates on facet activity **definitions** and is not reached by the maintenance registry at all, which discovers `apps/*/maintenance.py`.
- 📌 **Directory counts over-state these four by roughly 8×.** The six directories total 18,539 lines; the real owner-visible web surface across the four survivors is ~7,470, of which 3,878 is `timeline`'s JS and CSS that relocates rather than being rewritten — leaving ~2,278 lines of route code actually replaced by Rust.

**Two findings from web-surface conversion, 2026-08-14** — engineering findings, not a grouping decision:

- 🔴 **When an app's web UI is removed, its registry row must STAY at `converted: false`.** Measured on `activities`, whose six registered API routes back a native command grammar rather than the UI, so the UI can go while those routes must not. The row is what holds them behind the session gate: drop it and `known_app` returns `None`, the whole `/app/{name}` prefix becomes gate-exempt, and the create route writes owner records into a journal that was never established. ✅ With the row present, `POST` against an unestablished journal answers 302 and creates **zero** files. ⚠ Removing the row alongside the UI reads as cleanup and is a data-safety regression.
- 📌 **A frozen conformance corpus records the reference, so a deliberate app drop reads as a regression forever.** Dropping `reflections` makes the shell's app list diverge from the recorded corpus permanently, and that corpus cannot be regenerated once the reference tree it was recorded from is gone. Carry the divergence as a narrow, named exception at the point of comparison, one that asserts the row was present before it removes it, rather than editing a fixture that several conversions read.

⛔ **The `convey` shell is not an app plate.** It is `P-web` core and already has its crate.

🔒 **OPERATOR RULING 2026-08-09: convey/Flask is REPLACED OUTRIGHT, not proxied** — *"a minimum viable path through the stack that is all rust, no python, no flask"* — and breaking other surfaces is authorized while a lane stays in its own scope. ⛔ That retires proxy, bridge and WSGI-shim shapes for **all ten** `P-web-*` plates.

**Carry forward, from the first lane — each of these cost real time to find:**

- 🔴 **Check that something can SERVE a route before converting one.** `solstone-core-entities` carries a complete **3,427-line** `/app/entities/*` axum router that **nothing in the workspace calls** — it compiles, tests and gates, and no owner can reach it. So the first requirement is **the process**, not routes. ⚠ And building the server is not wiring it: `journal convey` still resolved to Flask after the Rust process shipped.
- 🔴 **The session gate has THREE outcomes, not two.** `journal_is_active` **raises** on a config that exists and cannot be parsed, and the reference answers a **500 in the owner's voice** — not the first-run wizard. A port written `unwrap_or(false)` tells an owner their journal was never set up, over an existing journal.
- 🔴 **A refusal that answers 2xx tells every client it succeeded.** The unconverted-app refusal shipped as 200; the shell evaluates a background body with `new Function` only when `response.ok`, so it executed refusal JSON as JavaScript on every page load. Nine more plates each serve refusals for their unconverted siblings — pin `!status.is_success()`, never the code.
- 🔴 **An oracle diff is not an acceptance.** Two frozen corpora agreed while the page was throwing. **Drive it in a browser.** ⚠ `networkidle` never fires — the shell holds an SSE stream open.
- ⚠ **An empty journal proves almost nothing for a read surface** — `/api/grid` answers 55 bytes. And normalize a corpus **by field path, never by shape**: a `^\d{8}$` rule eats every `day` value and both coverage bounds, so a port returning the wrong window matches.
- 🔴 **The Rust replacement may COMPILE your app's own files into itself, and no Python-side check finds it.** `solstone-core-convey-shell/build.rs` reads its crate-local `assets/speakers/` markup, script, and `copy.py` constants; the speakers native authority command is compiled into `solstone-core-sol-client` via `#[path]`. Deleting or editing the parsed asset source panics the build script, so relocate those assets into the crate and re-point `build.rs` before deleting the Python web surface.
- 🔴 **Run the import sweep in BOTH directions — and the action INVERTS depending on who the consumer is.** Sizing the plate asks *what lives in my directory that isn't mine*; scoping the cut asks *who depends on what I am about to delete*. A consumer in the **ingest pipeline / `think/` / `talent/`** means ⛔ *not your plate, don't touch it*; a consumer that is **another web app or a CI script** means ✅ *the module IS your surface, and severing the dependency is part of your cut*. ⚠ I reported **2** external consumers for speakers. There are **11**.
- 🔴 **`git grep <pattern>` searches the WORKING TREE — name the rev.** Mine was **291 commits stale** and answered confidently, with well-formed results and nothing in the output saying so. Use `git grep <pattern> origin/main`. ⚠ And a `from … import` sweep over production `.py` finds four of eleven at best: it cannot see **string module names** in contract registries, **test-to-test fixture imports** (another app's `conftest.py` imports your test fixtures — deleting them fails **that app's entire suite at collection time**), **lazy imports inside functions** (deferred, so no import probe proves them), or the Rust tree at all.
- ⚠ **Check that your GATE can execute your criteria.** `make ci` in this repo is **Rust-only and actively poisons Python** — `CI_FORBIDDEN_INTERPRETERS := python python3 pytest ruff uv`, shimmed to exit 97 before the run. Six Python acceptance criteria were gated on it; a green `make ci` would have been reported as the wave passing **while nothing it asserted had ever executed.** The Python checks run under `make install-checks`.

## `P-CLI`

Splits almost immediately.

| | Reach | May do |
|---|---|---|
| `P-CLI-sol` | any device | **API only**, over a link |
| `P-CLI-journal` | the same journal device only | **modify the journal directly**, or the API over localhost |

The split is enforced by separate installed executables. `solstone-core-sol`
contains the API client and link transport but has no journal filesystem,
configuration, socket, status, archive, facet, entity, or Python-process
authority. `solstone-core-journal` owns local journal resolution and the
same-device command root. Its archive, facet, and news mutations execute in
Rust; retained journal services are selected from a closed Rust process table.
There is no shared identity flag or Python CLI dispatcher that can change one
reach into the other.

## `P-system`

Operations for managing asynchronous activity — starting things, running things.

**Carry forward:** the task-request refusal **classifies rather than guesses** — it distinguishes `"wedged"` (runtime past a multiple of the partition's cap) from `"still_running"` and emits a skip event carrying **both** refs, the command, the scheduler name and the reason. A refusal that says *which* refusal it is.

⚠ **But the busy-partition branch is four-way, not two.** Before that predicate runs there is a bypass: a request carrying `queue_if_active_cmd_differs` whose command differs from the active one is **queued anyway** — no refusal, no event, no classification. It is the branch that decides whether work runs at all. And two further paths answer **nothing**: a request with no command, and a request arriving with no queue. A caller waiting on the skip event cannot distinguish *refused* from *never arrived*.

🔴 **The queue partition is an ordered resolver, not a lookup, and it is this plate's identity function** — it decides what serializes against what, which cap applies, and what a refusal collides with. `think` resolves by scanning a fixed flag order and taking the **first** hit; a production command carries two of those flags at once, so a set-membership port silently routes it to a different lane. `maintenance` sub-partitions only in one argv shape. Most partitions carry no registered cap and fall to the default.

⚠ **The command channel is modelled as unbounded argv and used as a seven-verb vocabulary.** Every production argv has head `journal` or `sol`. The one genuinely open door is the schedule config, whose `cmd` array is executed verbatim — an owner-editable file on the owner's own machine.

✅ **CONVERTED 2026-08-10 — this plate is native.** `solstone-core-system` holds the command channel (`request` · `partition` · `cap`), the task queue (`queue`), process lifecycle (`lifecycle` · `process`), the scheduler (`schedule`) and provider runtime (`provider_runtime`); `solstone-core-system-health` holds the health plane. They compose into `solstone-core supervisor [--journal PATH]`. **Three findings above were carried into the rebuild rather than re-derived** — the four-way busy branch, the ordered first-hit partition resolver, and the typed channel with exactly one named escape for the schedule config. ⚠ **A caveat that outlived the port:** the *"refused vs never arrived"* ambiguity is a property of the protocol, not of the old implementation — a caller waiting on a skip event still cannot distinguish the two, and a future contract change is where that gets fixed.

## `P-system-health`

Health of running things — current status, in-memory. ⛔ **System** health, never owner body data.

🔴 The per-day health JSONL grammar is **almost entirely Python string literals** — ⚠ *almost*, and the exceptions are the ones that matter. Three of the sets are already **imported from their owners** (the sensed-terminal states from the data-state enum; the deterministic-failure reason codes and the failure-cap predicate from the cogitate policy), so a rebuild that re-types them creates a cross-language fork with nothing binding it. What has **no owner anywhere** is the mode set — the writer derives the run mode from an argparse chain, not from a constant.

📌 **Measured: 18 record kinds on the writer side, and the reader consumes 10.** Eight are written on every run and read by no production consumer, though six of those eight are asserted on by existing tests. A typed schema forces that question; ⛔ do not answer it by quietly dropping them.

⚠ **The run log's identity story is real but NOT where it first appears to be.** ⛔ Corrected: the fold reads `mode` **from inside the record**. The filename-suffix derivation lives in the day-summary path and in two consumers that bypass this plate entirely and glob `*_daily.jsonl` directly — one of which already carries a comment admitting it will *"silently undercount"* if the shape changes. `ref` is written by one of the 18 kinds and **no reader anywhere parses it out of a path**. Fix the derivation where it is; ⛔ do not invent a filename fallback where it is not.

✅ **RESOLVED 2026-08-10 — `stale_heartbeats` is populated from real state.** It was hardcoded empty, and the finding was that shipping it empty is a claim the code does not back. The native snapshot derives it from the sync check: foreign writers filtered to `!is_live`. ⛔ It **fails closed** — a live writer is never reported stale — rather than defaulting to "nothing wrong." 📌 The carry-forward is the principle, not the field: *earned status fails closed*, and a published field that always reads healthy is indistinguishable from one nobody computed.

⚠ **The writer is fail-silent by construction** — a failed open logs once and every later write is a no-op. **So a run whose sidecar never opened is indistinguishable, to this plate, from a run that did nothing.**

## `P-system-models`

Model and runtime **artifact management** — what an owner's machine downloads, where it comes from, and whether the host can run it. Strand `S:system:system-models`.

✅ **APPROVED 2026-08-12** by operator ruling.

⚠ **The name under-reads the scope, deliberately, and a reader should know it.** The plate owns artifacts that are not models: the `llama-server` and `parakeet-server` binaries, the ced and rf-detr engines, and the Vulkan probe helper. "Models" is the established term for the thing and the family placement beside `P-system` / `P-system-health` is the point; ⛔ do not read the name as a boundary.

🔴 **The covenant property lives here, and it is structural rather than conventional.** Every Rust-side download resolves through `Artifact::origin_key` to sol pbc's own origin, enforced by a **single-element allowlist** inside the one fetch primitive (`install/archive.rs:14`, `DOWNLOAD_ALLOWED_HOSTS = ["updates.solstone.app"]`; `PRODUCTION_DOWNLOAD_POLICY` sets `allow_http: false`). `validate_url` refuses any other host with `HostRefused` and any `http` scheme with `InsecureScheme`, **per redirect hop**, under `MAX_REDIRECT_HOPS = 5`. A caller cannot forget it and there is no upstream host to fall back to.

⚠ **A denylist does NOT work here, and per-hop validation is why**: `github.com` 302s to `release-assets.githubusercontent.com` and `huggingface.co` to `us.aws.cdn.hf.co`, so a fallback reaching either makes zero requests to the denied names.

✅ **The macOS MLX fetch is native and resolves from our origin.** `OriginSnapshotSource` pulls each registry object through the allowlisted primitive and `run_mlx_install` defaults to it — `source_snapshot` absent means ours, present is the test seam, and there is no third option. `providers/mlx_install.py` still calls `huggingface_hub.snapshot_download` and `HfApi().list_repo_tree` directly, but nothing an owner runs reaches it; it is a retained reference, not a route. ⚠ **Superseded 2026-08-12** — this paragraph previously read *"the macOS MLX path still downloads from a third party today,"* which was true when written and closed the same week.

✅ **Every Rust fetch site resolves from our origin, including ced, rerank and rf-detr.** ⛔ **Retire the "12 of 20 flipped, the other 8 still upstream" reading** — it was true when the flip landed, before those three had native installers. Each now calls `download_artifact` → `download_verified` → `origin_key`. ⚠ Count this by **enumerating fetch sites, not rows**: the only artifact download site in the whole Rust tree is `install/archive.rs`'s primitive, so there is no second route a row could resolve through. Verified 2026-08-12 against a fresh checkout with a positive control.

✅ **The MLX snapshot set (macOS, 25 objects) IS in the registry as of `b925c27e5`** — that enrollment is what puts its 26.1 GB under the mirror tool and the deletion guard, since `current_mirror_targets()` derives from the catalog. ⚠ Its rows are deliberately absent from `pins::model_identity`, so no owner re-downloads.

🔴 **The macOS Core ML transcription models are the plate's remaining fetch, and half of it is closed.** On a Mac, transcription runs a Swift helper built on a pinned third-party audio package whose own downloader resolves models through a registry base URL — defaulting to a public model hub. Fetching from one tells that hub a journal exists, when it was set up, and that its owner turned transcription on.

- ✅ **The route is closed.** The helper pins that base URL to our origin before any call into the package. This is not the same as staging the models ahead of time: the package deletes its cache and re-downloads whenever a model fails to load, so a truncated or OS-invalidated model would otherwise send the retry upstream with nothing visible to the owner. A programmatic assignment also outranks both `REGISTRY_URL` and `MODEL_REGISTRY_URL`, so the environment cannot reopen it. Measured on Apple Silicon against a listener standing in for the hub: unpinned build 2 requests, pinned build 0 with models absent, 0 with them staged, 0 on the retry path.
- ✅ **The bytes are ours.** 23 objects, 483,254,213 bytes, registered under `unit: "parakeet-coreml"` and published to the origin, each corroborated against its upstream digest before publication and re-hashed after.
- ⏳ **The installer that stages them natively is not built yet**, so on `main` a Mac source install currently has no route to the models at all. ⛔ That ordering is wrong — the replacement should have landed first. No published build carries it.
- ⚠ **Where the models go is not where the cache directory points.** The package treats the directory it is handed as a *sibling* of the model tree, not its parent: models live at `<cache-dir>/../parakeet-tdt-0.6b-v3/`, and the readiness sentinel lives inside the cache directory. Established by running the helper both ways, because the in-tree comment on it is ambiguous and a synthetic fixture reads the other way.
- ⚠ **The readiness check is weaker than the package's own requirement** — it verifies four weight files where the package needs four complete bundles plus a vocabulary. A tree that satisfies the former and not the latter reads as installed and triggers the re-download path.
- ⚠ **Serving these bytes is redistribution of a *converted* work.** The Core ML build is a conversion of an upstream CC-BY-4.0 model, and that licence — unlike MIT or Apache-2.0 — obliges indicating modification as well as attributing. The notices file owes both.

📌 **Change what is FETCHED, never what is RECORDED.** `pins::model_identity` still records `"revision":"main"` while the fetch moved to a pinned sha, because `prove_manifest` compares identity by exact canonicalized-JSON equality — a changed field re-downloads gigabytes silently, with bandwidth as the owner's only signal. `check_version` rejects `"main"`, so the disagreement is permanent and correct.

✅ **The journal `warm` verb and its Python-payload contract are retired.** Owner-facing `journal warm` no longer exists; the dispatcher no longer resolves an interpreter to load extension modules. Native `solstone-core warm` is a different verb and is not this plate.

## `P-distribution`

**How the journal reaches a machine.** ⛔ **Three things, not one: the tool that PRODUCES the artifacts, the artifacts themselves, and the path an owner installs them by.** All three are inside this plate.

🆕 **Added 2026-08-16 by operator ruling.**

🔴 **The boundary, and the only reason this plate exists:** ⛔ **no Python anywhere in producing, publishing or installing the product.** Not in the artifact, and not in the machinery that builds it. An owner installs the journal and runs it; nothing has to be on the machine first.

⚠ **This is narrower than the general build-tooling rule and deliberately so.** *"Python in build-time tooling is fine"* holds across the tree; ⛔ **it does not hold here.** A distribution produced by a Python toolchain cannot be verified by an instrument that has no interpreter, so the producer and the artifact stand or fall together.

🔴 **A done-condition satisfiable on a host that already has Python does not test this plate.** Every other boundary is graded on a dev checkout or an installed wheel, and both of those already have an interpreter — which is how the delivery layer stayed unowned while every plate went green. ✅ **The instrument runs on a host with no interpreter present, against a control on a host that has one**, so a zero can be distinguished from a blind probe.

### The producer

**One Rust binary over a declarative inventory, emitting every container from one staged tree.** ⛔ Not a script per format, and ⛔ not a Python build backend wrapping native output.

✅ **The shape already exists in this workspace and is days old** — `solstone-ci` (`core/crates/solstone-core-repository-contracts/src/bin/solstone-ci.rs`) drives `core/ci/*.toml` and is invoked from `make` as `cargo run -p … --bin solstone-ci`. The distribution producer is that pattern pointed at packaging.

✅ **Every dependency it needs is already a workspace dependency, used in shipped crates:** `tar` and `flate2` both read and write gzipped tarballs today (`solstone-core-transfer/src/export.rs` writes, `solstone-core-local/src/install/archive.rs` reads); `sha2` digests; `ureq` fetches. ⛔ **No external packaging toolchain is permitted** — a `.deb` is an `ar` container over two tarballs the workspace can already write, and an `.rpm` is produced by a pure-Rust builder. `dpkg-deb`, `rpmbuild`, `maturin`, `setuptools` and `twine` are all outside this plate.

### The artifact

**One relocatable tree per (os, arch). Every container is a wrapping of that tree, never a separate build.**

```
bin/    journal · sol · solstone            POSIX-shell launchers, $0-relative
        solstone-core · solstone-core-journal · solstone-core-sol
        solstone-core-describe · solstone-core-depict
        solstone-core-speakers-analyze · solstone-core-vulkan-probe
        solstone-retention
lib/    solstone-core-speakers-analyze/libonnxruntime.so.1
        solstone_journal_models/assets/*.onnx
share/  notices and licences
```

✅ **Shipped binaries resolve this layout through the third candidate in `resolve_installation_root_from_executable_dir`:** payload lives at `share/solstone/**`, the resolver returns `<prefix>/share`, and tar/deb/rpm normalize to that tree. That property is what the plate is built on, and each half of it is a fact about code in this tree rather than an intention:

- the three launchers walk `$0` symlinks and `exec` a native sibling resolved from their own directory (`scripts/root-launchers/`) — ⛔ nothing in them is virtualenv-aware
- `libonnxruntime.so.1` is reached by the rpath `$ORIGIN/../lib/solstone-core-speakers-analyze`, emitted by `core/crates/solstone-core-speakers-analyze/build.rs`, over bytes staged from the pinned-digest table in `core/crates/solstone-core-distribution/src/onnx_runtime.rs`
- model assets resolve at `<ancestor-of-exe>/lib/solstone_journal_models/assets` and that candidate is tried **before** any `site-packages` path (`core/crates/solstone-core-transcribe/src/model_assets.rs`)
- `solstone-core` is fully static; every other binary links base system libraries only

🔴 **The installation root has one resolver and three ordered layouts.** `resolve_installation_root_from_executable_dir` first preserves an installed `site-packages` containing `solstone/__init__.py`, then a git checkout carrying `pyproject.toml` + `.git` + `solstone/`, then a distribution tree whose sibling `share/` contains all three exact anchors. The distribution candidate returns `<prefix>/share`; a partial or lookalike tree is rejected. The resolver tests in `core/crates/solstone-core-journal/src/lib.rs` cover precedence, relocation, exact anchors, and negative twins.

⚠ **First-run paths including `journal setup`, cortex, and the talent runtime depend on this central resolver.** Production consumers delegate to it rather than maintaining per-crate fallbacks. `distribution_no_independent_resolvers` enforces that ownership across the repository.

📌 **The resolved payload is data, not code.** The individual-path allow-list in `core/distribution/payload.txt` admits talents, prompt templates, contract data, and AMD attestation roots, which the producer stages at `share/solstone/**`. Every current source path remains an input until the inventory and source location move together.

⚠ **This is also a warning about how the plate gets tested.** The obvious smallest-loop proof — capture → segment → index → findable → reads back — **does not reach cortex or the talent runtime**, so an artifact can pass it with thinking dead. ✅ **A tree is not proven until a talent runs from it.**

⚠ **The dispatcher binary must sit beside the running executable, not merely on `PATH`.** The sense dispatcher spawns `[<abs>/solstone-core-journal, "describe", …]` with no `PATH` fallback, so an install that omits the co-located dispatcher makes **every** handler fail closed as a per-file segment error naming the candidate path rather than one. See `P-segment-sense`.

**Containers, all derived from the one tree:** `.tar.gz` is the primitive · `.deb` and `.rpm` relocate it under a system prefix and put the three launchers on `PATH` · macOS takes the same tree signed and notarized. ⛔ Windows is unsupported and is not a container.

🔴 **A Python wheel is not one of them.** A native core delivered inside a `.whl`, unpacked by a Python package manager into `site-packages`, and gated at runtime on Python package metadata is a native core wearing Python packaging. ⛔ **Thinness is not the test; delivery is.**

### The install path

**The default path is a one-command bootstrap that assumes nothing but a POSIX shell and a fetcher**, in the shape every native tool ships: detect `(os, arch)`, fetch that tree, verify its digest, extract it under a prefix, put `bin/` on `PATH`.

⚠ **A shell bootstrap is not a violation of this plate — an interpreter the product does not ship is.** `sh` is on every supported host by definition; Python is not. ⛔ Keep the shell to detect-fetch-verify-extract; anything that needs judgment belongs in a Rust verb the tree already carries.

✅ **The origin is already ours and already covenant-enforced.** `solstone-core-artifact-download` pins a single-element allow-list (`updates.solstone.app`), refuses any other host with `HostRefused` and any `http` scheme with `InsecureScheme`, **per redirect hop**. ⛔ Distribution does not introduce a second origin, and a fetch path that cannot be expressed through that primitive is a design error rather than an exception.

⛔ **The owner-facing install text is part of this plate, not documentation about it.** `INSTALL.md`, `solstone-core-check`'s closing line, and every doctor remediation string name an installer; they are wrong the moment the container changes, and they reach owners before anything else here does.

### Carry forward, non-negotiable

- 🔴 **One artifact, one version.** Every binary in a tree comes from one build of one commit. The failure class *"a thin client upgraded out of step with a leaf"* — which `core/crates/solstone-core-journal-cli/src/coherence.rs` exists to detect, by reading `site-packages/*.dist-info/METADATA` before **43 of 46** process verbs — becomes **unrepresentable** here rather than merely detected. ⚠ Two properties of that check matter to whoever removes it: it **self-disables** when no `site-packages` is found, so it does not block a tree install, it is inert weight there; and its escape list `KNOWN_SOLSTONE_PACKAGE_NAMES` (`core/crates/solstone-core-journal/src/lib.rs:77`) names **none** of the `solstone-core-*` family that every current install has, while `target_from_dist_info_directory` (`:401`) claims their dist-info directories by prefix — so on a wheel install it refuses those verbs.
- 🔴 **Nothing in the artifact is test material.** `setup-fixture-journal`, `solstone-transcribe-*-stub`, `solstone-describe-*-stub`, `solstone-generate-*-stub` and the `*-test-child` helpers are plain `[[bin]]` targets with **no `required-features`**, so a default release build produces them and any packaging step that ships *"the binaries"* ships those too. The inventory is an allow-list, never a directory listing.
- 🔴 **The install-time copy an owner reads belongs to this plate.** `solstone-core-check` now closes with *"see INSTALL.md"*. Ten of the doctor's checks are still about the retired Python install layout — `python_version` · `sol_importable` · `host_dependencies` · `journal_leaf_exclusivity` · `journal_package_version` · `retired_host_shim` · `package_metadata` · `local_bin_sol_reachable` · `stale_alias_symlink` · `disk_space` — and several still render `pip install` / `pipx` / `uv tool` remediation. A packaging change that leaves those checks is a native product diagnosing a layout it no longer has.
- **Model bytes are either inside the artifact or fetched from our origin — never both, and never neither.** See `S:distribution:system-models`.
- ⚠ **The instrument that tests an install must be able to fail.** `core/distribution/cleanroom.sh` is the live oracle; `scripts/cleanroom-install.sh` is the retired Python-only harness and is structurally incapable of testing a host without an interpreter.

## `P-body-source`

Owner **body** data arriving from outside: Oura and Apple Health. ⚠ **Ingress, not egress.** This body-import path uploads no body records or other journal content.

✅ **CONVERTED 2026-08-09.** `solstone-core-body-ingest` owns bounded Apple parsing and Oura OAuth/network/cursor ingress; `solstone-core-body-source` owns the versioned normalized row, hash, manifest, envelope, and ledger contracts; `solstone-core-body-store` + `solstone-core-body-rebuild` own replay, retained-raw verification, and atomic dedupe publication. The Python Apple/Oura modules retain only independent read/normalization oracles exercised by a Rust-hosted full-corpus differential, and `body_native.py` is process transport. There is no Python body writer, Oura network/token/cursor owner, or dedupe writer.

📌 `imports/health-dedupe.sqlite` remains deliberately excluded by the backup engine's `*.sqlite*` rule because it is derived state. Immutable `imports/body-<ULID>/` bundles are backed up; restore rebuilds SQLite from their validated envelopes, ledgers, normalized shards, and any digest-bound retained-raw inventory before it persists recovery state or reports success. A real-restic synthetic Apple+Oura round trip reproduced every dedupe field exactly and restored the retained Oura API pages; an invalid native bundle or changed retained asset makes restore fail closed rather than silently returning empty history.

---

## ⛔ Egress — where the covenant applies

Journal and devices are **one secure environment**. No per-plate privacy tracking inside it; transport security is already covered.

**Actual egress — three:** `P-BYO` (the owner's own key, owner-directed) · `P-SPP` (attested, non-retained) · **support requests** — ⚠ the `_SECRET_*` redaction in `apps/support/diagnostics.py:29-50` is the **last thing** between a journal config and an external service.

**Blind by construction, therefore not egress:** relay transit · push notifications · encrypted backups.

🔴 **Push is only reimplemented in an end-to-end encrypted form** — the journal encrypting and the receiving device decrypting with the link cryptographic identities. ⛔ The current plaintext path, which carries journal-derived chat content to a push service and which unpairing does not revoke, does **not** come across.
<!-- historical; push paused, chat trigger retired — future payload is journal state / device check-in -->
