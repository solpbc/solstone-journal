> Historical. This document predates the chat removal (2026-08-20) and describes conversion planning against a tree that still had chat. Treat strand status as of that snapshot, not current.

# strands — the work units

**A strand is the minimum viable path connecting two plates, in Rust, robust.** It may be bi-directional; its contract lives at exactly **one end**, and by convention that end is written second in the name.

Definitions and vocabulary: [`README.md`](README.md). Siblings: [`plates.md`](plates.md) · [`cables.md`](cables.md).

**Read your strand's section, not the file.** Organized for surgical section edits.

⚠ **Every count here is a floor, not a census** — see `README.md` on the two string-keyed dispatch registries that defeat counting.

⛔ **Reserved words:** `health` = journal system health only; owner physiological data is `body`. `activities` = the internal facet model only.

---

## Tier 1 — the core

### `S:device-link:journal`
**Connects** `P-device-link` → `P-journal` · **Owner** `P-journal` · **Tier** fixture + schema

Link identity. A device proves who it is; the journal decides whether it is authorized.

⚠ **24 production (non-test) modules import `solstone.think.link`**, which makes it the least isolable thing in the tree — an argument for converting it early rather than deferring it.

The identity derivation, the two fingerprint kinds, the `did`, and the mark's exact parameters are in [`plates.md`](plates.md) § `P-device-link`. ⛔ Do not restate them here.

**Carry forward:** the certificate fingerprint is the identity, the label is only a label · revocation is the ledger, not the certificate · **an unreadable `authorized_clients.json` authorizes nobody** · loading never rewrites the ledger.

### `S:device-ingest:segment-media`
**Connects** `P-device-ingest` → `P-segment-media` · **Owner** `P-device-ingest` · **Tier** schema

Segments arriving from a device. This is the **ingest envelope** — a wire shape, not bytes on disk. 🔴 **CORRECTED 2026-08-19 — the citation was stale.** This entry named `observe/protocol.schema.json`, deleted along with the rest of the Python `observe/` tree in the conversion. The live, actively-validated (via `jsonschema` in `core/crates/solstone-core/src/contract/validate.rs`) successor is `core/crates/solstone-core/src/contract/schemas/protocol.schema.json`, confirmed by content match and by its membership in `contract/bundle.rs`'s `REQUIRED_SOURCES`; it carries `file_kind: "ingest_envelope"` and `producer_write_paths` of the ingest endpoint.

🔴 **CORRECTED AGAIN, same day — the schema's CONTENT was also stale, not just its path.** It required legacy `stream`/`observer` fields that the live handler (`solstone-core-ingest`'s `parse_envelope`) unconditionally refuses, omitted the `source` field the handler actually accepts, over-required `files[]` metadata the handler strips unvalidated, named the pre-rename route `POST /app/observer/ingest` (a 404 since the devices fold), and carried `schema_owner: "observe.protocol"`, a retired Python module. Corrected against the router code: `required` is now `["day", "segment", "files"]`, `source` is documented, `propertyNames` explicitly forbids `stream`/`observer` so `additionalProperties: true` can't silently re-accept them, `producer_write_paths` now names `POST /app/devices/ingest`, and `schema_owner` is `solstone-core-ingest`. A duplicate, unwired crate-local draft (`observer-ingest-envelope.v2.json`) that would have permitted the same legacy fields was deleted rather than left as a second, disagreeing description. The published bundle artifact (`core/payload/solstone/talent/journal/contract/bundle.json`) was regenerated via `journal contract build` to match.

⛔ **The client no longer asserts its own identity.** The journal authenticates the certificate at the transport, derives the `did`, and records it. Nothing routes, authenticates, or names a stream from a client-supplied string.

**Carry forward:** create-exclusive byte writes that never overwrite — on collision compare SHA-256 and record the already-held case · content identity **refuses rather than guesses** · the segment-key timezone semantics — `HHMMSS` is **device-local wall clock, not UTC**.

⚠ The audited client bundle and the plate's declared operations are **different sets**. The plate publishes **9** `observer.*` operations; the bundle carries **5** of them and adds **3** belonging to other plates, so the capture clients contractually depend on chat, pairing and callosum. The **4** it omits are `deleteSource` — the owner's location-data delete, the covenant-critical one — plus `health`, `ingestManifest` and `ingestManifestDay`.

⚠ **A device may own several streams, so the `did` alone cannot select one.** A watch app relaying through a phone presents the **phone's** certificate and therefore the phone's `did`; no other signal in the envelope separates them. The envelope carries a **`source`** — a short, device-chosen, stable sub-stream discriminator, empty for the device's primary capture. ⛔ It is not identity and nothing authenticates with it: the journal resolves `(did, source)` to a stream it already owns. A client can only say *which of its own* streams this is.

### `S:segment-media:journal-segment`
**Connects** `P-segment-media` → `P-journal` · **Owner** `P-journal` · **Tier** fixture

Raw media landing durably, and the **segment sidecars**.

**`device.json`** — optional; when present it carries at minimum the **`did`**, plus optional ephemeral device metadata the device supplies (battery level, screen lock, posture) as **free-form fields opaque to the journal and validated only for the required `did`**.

⛔ **`device.json` is journal-authored, never client-uploaded.** Reserved sidecar names are never written from client bytes and never appear in segment listings as held. The device supplies its fields as data over the API; the journal writes the file.

🔒 **Nothing is ever looked up by stream name. Attribution always reads the segment.** The stream name is a human-friendly, unique, cross-platform-filesystem-safe label — a deterministic projection of a display name like `iPhone (2)` to **`iphone_2`**. One device may own several streams (a watch app via a phone is a different stream). ⛔ **The `iPhone_2` spelling is retired**: the existing rule is `^[a-z0-9][a-z0-9._-]*$` — lowercase only — and a mixed-case projection does not match it.

✅ **Uniqueness is case-insensitive by construction, not by comparison.** Casefolding before the charset filter means two labels differing only in case produce the *same* string, and the ordinal allocator resolves them as an ordinary collision. There is no comparison rule for a later edit to drop, and APFS, NTFS and ext4 behave identically. The projection also excludes `<>:"/\|?*`, trailing dots and spaces, and the Windows reserved device names.

⛔ **The projected name never contains `.`** — the dot is the legacy kind separator and `import.` is a live prefix, so a projected `my.phone` reads as a qualified name. ⚠ **The name is not required to be invertible**, only unique: two different labels may project to the same string and the second takes an ordinal. That is what "the name is a label" buys.

🔴 **Uniqueness must be allocated over the PROJECTED name, never over the display label — the projection is many-to-one.** `my.phone` and `my_phone` both project to `my_phone`; `café` and `cafe` both project to `cafe`; and a device labelled `iPhone` taking ordinal 2 projects to exactly what a device labelled `iPhone (2)` at ordinal 1 projects to. Allocating on the label leaves all three colliding on one directory and one stream record, which means one **chain** — and duplicate grouping walks that chain into an irreversible delete. ⚠ The taken-set is `journal/streams/*.json` **plus** the authorization registry: the registry alone cannot see stream directories that predate it, and a `chronicle/` directory can outlive its record.

⛔ **RETIRED 2026-08-05 by operator ruling — do not re-derive it.** This entry used to say that a new sidecar name silently widens what the owner's delete removes, because the delete classified a segment as *location-only* or *mixed* using `RESERVED_SEGMENT_FILENAMES` as its non-content set. **The disposition is gone**: the segment is the unit of deletion, a segment is removed whole or not at all, and no name set decides what is kept. A mixed classifier returns only as a receipt cost-disclosure on that whole-segment erase. See [`plates.md`](plates.md) § `P-journal-retention`.

📌 **What the retirement is worth keeping.** The divergence was real and was measured before the rule changed: Python's set is 3 names (`think/segment_files.py`) against the segment crate's 7, which produced *different delete outcomes on the same journal* — `segments: 0 / mixed: 6` in Python against `segments: 3 / mixed: 3` in Rust. ⚠ The set still matters for what a client may not upload (`is_reserved_name`), and **`device.json` and `tombstone.json` are in neither Python's reserved set nor `is_structural_derived_file`**, so `apps/observer/prune.py` refuses any segment carrying one as a `derived-output` unknown. That arms at cutover. `events.jsonl` is fine — structural-derived covers it by name.

⚠ **`stream.json` is read far more widely than the plate's own notes suggest — 17 production call sites across 8 modules**, led by `think/segment.py` (6), `apps/settings/maint/*` (4) and `apps/observer/prune.py` (3), plus `think/cluster.py`, `think/streams.py`, `think/indexer/journal.py` and `observe/sense.py`. ⚠ **The 17/8 count predates the sense conversion and has not been re-measured.** `observe/sense.py` is deleted (2026-08-13); the native `solstone-core-sense` reaches segments through `solstone_core_journal_io::iter_segments` and never names `stream.json` itself, so at least one of the 17 has moved behind a crate boundary rather than disappearing. ⛔ Do not cite 17/8 as current — re-measure before relying on it, and note the Rust side must be counted too. 📌 An earlier count of "six sites in `prune.py`" was counting error-message string literals, not readers. It matters because the marker carries `prev_day`/`prev_segment`/`seq`, and the reference writer **resets `seq` to 1 and stamps a fresh `created_at` whenever the stream record fails to parse** (`think/streams.py:214-232`, a swallowed `JSONDecodeError | OSError`) — forking the chain for all 17, silently, on a path that looks like a successful write.

🔴 **The content-identity reference refuses in three cases and silently degrades in the rest.** `_read_ingest_manifest` returns `{}` for *every* failure — unreadable, not JSON, non-dict root, unknown `schema_version`, non-dict `files`, or any entry that is not an object — and `content_identity_from_segment` reads that as falsy and **falls through to the legacy media-scan arm**, producing a valid identity computed from whatever is on disk. One of its own refusals is unreachable as a result. ⛔ **A rebuild must refuse instead**: exists-and-will-not-parse raises, missing returns defaults. The two refusals that genuinely protect owner media — on-disk bytes disagreeing with the manifest's `sha256`/`size`, and a manifest-named file absent without terminal processing proof — the reference does get right, and both must survive.

**Carry forward:** **unresolved error → hold raw**, via `exit 69` — the handler writes nothing, leaves the input, and the scanner records *neither* success nor failure. The deferral emitter deliberately swallows its own bus failure so a down bus cannot turn a deferral into data loss.

### `S:segment-sense:journal-segment`
**Connects** `P-segment-sense` → `P-journal` · **Owner** `P-journal` · **Tier** fixture

Processed sense output written back.

⚠ **The `_solstone_processing` header moved out of this strand 2026-08-05** — it is now `S:segment-sense:segment-processing`, below. What stays here is the analysis output itself: `<stem>.jsonl` rows and the `<stem>.npz` speaker-embedding sidecar. 🔴 **CORRECTED 2026-08-19 — the citations were stale.** This entry named `observe/screen.schema.json` and `observe/transcribe/audio.schema.json`, both deleted along with the rest of the Python `observe/` tree. The live, actively-validated successors, confirmed by content match and `contract/bundle.rs` `REQUIRED_SOURCES` membership, are `core/crates/solstone-core/src/contract/schemas/screen.schema.json` and `core/crates/solstone-core/src/contract/schemas/audio.schema.json`.

⚠ Both of those schemas **under-declare their own headers**, and both carry `additionalProperties: true`, so nothing ever fails validation. Undeclared but historically written by retired `describe.py:653-657`: `_solstone_thinking`; audio's live fields are `overlap_fraction`, `overlap_detector`, `device`, `compute_type`, `speaker_analysis_producer`, `noisy_rms`, `noisy_s`, `loud_windows`, `speech_loud_windows`, `loud_speech_ratio`.

🆕 **The audio record gains `sentence_id` (additive), and the audio analysis-output writer is `P-speaker-id`'s to build — operator call, 2026-08-07.** Rule 4a's remaining instance is this strand's `<stem>.jsonl` row: `build_statement` mints a 1-based ordinal and `_statements_to_jsonl` discards it, so six sites re-derive it from file position and two durable stores persist it as a join key. Persisting it needs a native writer, and "no more Python ships" forbids the one-line Python edit — so `P-speaker-id` builds the native writer for the analysis output (`<stem>.jsonl` rows **and** the `<stem>.npz` sidecar). ⛔ **Strand ownership does not move: `P-journal` still owns this contract.** `sentence_id` goes into `$defs.record.properties` and `x-journal-contract.key_fields`, ⛔ **never into `required`** — the reference journal's 200,583 existing rows all lack it and must stay valid.

✅ **SHIPPED 2026-08-09 — correction to the present tense above:** `_statements_to_jsonl` is deleted. The transcribe handler now routes through `solstone-core speaker-transcript-write`, and every newly written transcript row carries its minted `sentence_id`; it is no longer discarded. The six read-side re-derivation sites and the ASCII-escaping hazard above remain live facts.

🔴 **Carry forward — the native row writer MUST escape non-ASCII.** `json.dumps` defaults to `ensure_ascii=True`; `serde_json::to_string` does not escape at all. The six `sentence_id` derivation sites use **three different line-splitting primitives** (`readlines()` under universal newlines · `str.splitlines()` over Unicode line boundaries · Rust `str::lines()` on `\n` only), and a **raw U+2028/U+2029/U+0085/U+000B/U+000C/U+001C–U+001E inside a text value splits the row for one of them and not the others** — `edges.py` drops two sentences and renumbers the rest. ⛔ A plain `serde_json::to_string` for this file silently shifts sentence ordinals. ⚠ A **lone `\r`** line ending is the mirror case: Python reads the rows, Rust reads the whole file as one line and emits nothing. ✅ All three agree that a blank or unparseable line **consumes** an ordinal, so a legacy positional fallback must count them.

**Carry forward:** **terminal-empty written before the raw is released** — die between them and the file survives with a terminal marker, never the reverse · atomic promote through a same-dir temp, header **last** · detections stored raw, filtered at read time.

⚠ **Three scope corrections to those three, all verified by reading the code:**
- 🔴 **The unlink half of terminal-empty is RETIRED 2026-08-05 by operator ruling — the marker discipline is not.** `transcribe` writes the terminal-empty marker exactly as it does today (`transcribe/main.py:1182-1191`, `:810-819`; durable because `write_text` fsyncs the temp, `os.replace`s, then fsyncs the parent) and then **hands the raw to `P-journal-retention` instead of calling `unlink()`** (`:1209`, `:836`). ⛔ Retention is the only plate that removes owner media. ⚠ The *ordering* invariant survives intact and still matters — the marker must be durable before the raw is handed over, never after. The discipline was transcribe-only in any case: describe writes the empty marker and never unlinked the video.
- **"Header last" is true of the *decision*, false of the *byte order*.** In the retired Python reference's promoted file the header was physically line 1; what happened last was *determining* it, inside `_promote` (`describe.py:896-925`), once the run knew its verdict. ⛔ A rebuild that appends the header at the end of the file has read this backwards.
- **Detections are filtered at exactly one read site** — `qualified_objects` (`observe/detect.py:106-122`) has a single production caller, `observe/screen.py:225`. ⚠ So `depict`'s stored `source="still"` detections are **never** filtered on any read path.

### `S:segment-sense:segment-processing`
**Connects** `P-segment-sense` → `P-segment-processing` · **Owner** `P-segment-processing` · **Tier** fixture

🆕 **Added 2026-08-05 by operator ruling**, split out of `S:segment-sense:journal-segment`. The per-file outcome ledger — `_solstone_processing` — and the predicates every reader decides against. See [`plates.md`](plates.md) § `P-segment-processing` for why it is its own boundary.

🔴 **There are THREE producers, and the third is the one that matters most.** ⚠ This line previously read *"describe and transcribe only"* and then named a third in the same sentence — a self-contradiction that is exactly how the third gets omitted downstream.

| producer | writes | via |
|---|---|---|
| retired Python `observe/describe.py:911-918` | screen verdicts | `build_processing_record` (`observe/processing_record.py:160-190`) |
| `observe/transcribe/main.py` at **`:791-796`** (empty), **`:1147-1152`** (failed), **`:877-882`** (analyzed) | audio verdicts | same ⚠ `:631-632` is the metadata-assembly line, not a call site |
| 🔴 **`think/backfill_processing_records.py:170-198`** | `state=empty` with `source="backfill"`, and `input_size` from the media sibling — **or `0` when the sibling is absent or `stat()` raises** | same |

🔴 **The third stamps a verdict no handler produced, and both terminal-proof implementations ignore `source` entirely.** So an operator-stamped `empty` licenses retention to purge and licenses a device to drop its only copy, with nothing having processed the file. ⚠ Measured: a real journal's screen outputs that carry **no record at all** — and therefore cannot grant proof — are precisely this tool's target population, so one operator command converts them from proof-less to proof-bearing. ⛔ `source` is the record's only provenance field; a rebuild must carry it, and must not quietly resolve this.

⚠ `core/crates/solstone-core-backfill-cli/` is the live native implementation of the same backfill producer, reached through the `backfill-processing-records` native process token; `think/backfill_processing_records.py` is its retained reference. The native path skips an unmeasurable media sibling instead of stamping `input_size=0`, classifies a torn JSONL row or non-UTF-8 input unreadable rather than empty (the reference swallows a per-line JSON decode error and can stamp a torn row), and skips the current day. ⛔ The exposure is narrowed, not closed: neither terminal-proof implementation nor retention reads `source`, so an operator-stamped `empty` still licenses a purge and readers cannot distinguish it from a handler verdict.

⚠ **`observe/depict.py` writes no record at all**, so image outputs are invisible to every reader — and both terminal-proof implementations refuse any extension outside audio/video anyway (`apps/observer/processing_proof.py:26-33`, `core/crates/solstone-core-processing-record/src/media.rs:40-58, :82-89`), so **an ingested image can never be proven consumed** and the sending device never releases its local copy. 🆕 **Operator ruling 2026-08-05: `depict` is promoted to first class** — it gains a record, a schema, re-entry and a formatter entry, and this hole closes.

**The 9 production read sites, measured** — ⚠ "read by five planes" was a floor:

| Reader | Decides |
|---|---|
| `core/crates/solstone-core-sense/src/batch.rs:226-227` (retired Python `observe/sense.py:1066`) | whether a batch scan re-enters a file — `should_reenter_analysis_output(read_processing_record_header(…))` |
| retired Python `observe/describe.py:1577`, `:178` | handler self-skip, and which rows an incremental re-run reuses |
| `think/data_state.py:54` | the shared modality state, consumed by `think/cluster.py:503` and `apps/transcripts/routes.py:747` |
| `think/retention.py:135-142` | 🔴 **irreversible raw-media deletion** |
| `apps/observer/processing_proof.py:61` | 🔴 **that a device may drop its local copy** |
| `core/crates/solstone-core-ingest-resolve/src/terminal_proof.rs:53` | the same, in Rust |
| `think/backfill_processing_records.py:158` | its own skip guard |

✅ **One version string in the Rust tree** — `vocab::SCHEMA` (`core/crates/solstone-core-processing-record/src/vocab.rs:5`) is the only Rust declaration, and a committed test `include_str!`s `terminal_proof.rs` to assert neither the literal nor the old `PROCESSING_SCHEMA` name can come back (`core/crates/solstone-core-processing-record/src/predicate.rs:85-90`). ⚠ **Cross-language equality with `processing_record.py:23` is still confirmed by inspection only** — nothing mechanically enforces it, and that is an accepted residual until Python is deleted.

⚠ The two terminal-proof readers require more than the schema does: `processing_proof.py:64-76` and `core/crates/solstone-core-processing-record/src/predicate.rs:26-34` both check `schema` match, `state ∈ {analyzed, empty}`, `handler` matching the extension, **and `input_size == recorded_size`**. That conjunction is the contract for releasing owner data, and it is written down in neither place.

🔴 **But the conjunction is NOT what gates the other irreversible decision, and reading it as though it were is the trap.** Measured 2026-08-05 by tracing `think/retention.py:135-142` into `think/data_state.py:121-158`: the retention path reads **`record.get("state")`, plus `is_failure_exhausted(record)` on the failed branch, and nothing else.** ⛔ No `schema` check, no `handler` check, no `input_size` check. **A record consisting of `{"state": "empty"}` and nothing else is enough to purge an owner's raw media.** The stricter predicate protects the device's copy; the weaker one deletes the journal's.

🔴 **And the two terminal-proof implementations already diverge, in the direction that releases owner data.** Condition 3 is parsed differently on each side — Python uses `Path(name).suffix` (`processing_proof.py:26`), Rust uses `name.rsplit_once('.')` (`terminal_proof.rs:69-70`). The extension *sets* match exactly; the *parsers* do not. For a name whose only dot is leading — `.mp4` — Python's suffix is `""` and it refuses at the first branch, while Rust yields `"mp4"` and proceeds to grant proof. ⚠ **Reachable:** `ContentName::new` accepts `.mp4` (`solstone-core-segment/src/content_name.rs:41-55` rejects only empty, `/`, `\`, `.`, `..` and reserved names) and it is built straight from the client-submitted filename (`solstone-core-ingest/src/router.rs:325-330`, `:427-431`). So a device can be told it may delete its only local copy on a name the reference would never have proven. ⛔ **A rebuild narrows to the reference here; widening proof is the one direction that loses owner data.** 📌 The sub-shape worth carrying: Python holds the extension sets **dotted** (`think/media.py:8-33`), Rust holds them **undotted** in a linearly-scanned slice — `FORMATS` (`solstone-core-processing-record/src/media.rs:40-58`), matched by `media_kind` (`:63-69`) — two representations of one set, which is why the parsers could drift without anyone noticing the sets had not.

⚠ **Re-entry is describe-only in practice.** `should_reenter_analysis_output` (`processing_record.py:118-152`) returns `True` only for `handler == "describe"`, and transcribe's own decode-failure writer then blocks re-entry at three separate guards — so `FAILED_ATTEMPT_BOUND` (3) never applies to audio. ⛔ Whether that asymmetry is intended is a contract question this strand owns, not a bug to fix silently: `tests/test_data_state.py:145` encodes it as deliberate, while the *other* transcribe failure paths write no record at all and re-pay decode + VAD + STT forever. Rust re-entry now covers `{describe, depict}`; transcribe still stays out.

**Carry forward:** the closed sets are the contract — `state ∈ {analyzed, empty, failed}`, `reason_code ∈ {ok, no_decodable_frames, no_decodable_audio, corrupt_input, analysis_failed}`, `handler ∈ {describe, transcribe}` — and `corrupt_input` is terminal immediately while everything else exhausts at `attempts >= 3`. The Rust vocabulary since this change declares `{describe, transcribe, depict}`, with depict's analysis row key `"text"` (`IMAGE_ANALYSIS_ROW_KEY`).

### `S:segment-sense:system`
**Connects** `P-segment-sense` → `P-system` · **Owner** `P-system` · **Tier** schema

Callosum events emitted as processing happens.

There **is** a published machine-readable registry — `CALLOSUM_REGISTRY` (`convey/contract/assemble.py:43-99`), emitted as `x-callosum-registry`, plus two published SSE operations (`callosum.rootEvents` at `convey/root_contract.py:35-93`, `observer.callosumStream` at `apps/observer/contract.py:602`).

⚠ **CORRECTED 2026-08-05, counted by importing the dict: 11 tracts and 58 pairs — 57 concrete event names plus the `notification: ["*"]` wildcard**, not ~60. The same 11/58 is reproduced by the generated fixture `core/fixtures/callosum_registry.json`.

🔴 **The registry is not a closed vocabulary and does not claim to be, so "drift" against it is the wrong frame for a rebuild.** `assemble.py:363-371` publishes it as `x-vocabularies["callosum.tract_event"]` marked `classification: extensible` / `unknown_value_behavior: preserve`, and the `CallosumEvent` schema (`:313-322`) puts **no enum on `tract` or `event`**. ⚠ **CORRECTED 2026-08-05: ONE open relay, not two.** `apps/observer/routes.py:1374-1387` republishes an authenticated remote observer's own `(tract, event)` verbatim with no allowlist, so the vocabulary is structurally un-enumerable. ⛔ `convey/chat.py:578` is **not** a second one — it returns early unless the tract is `cortex` (`:505`), denylists `event == "request"` (`:532`), and re-emits with the tract **pinned**. It relays the *event half only, into a fixed tract*. A rewrite that models it as pass-through gets the cortex pinning wrong. ⛔ **A Rust rewrite must preserve unknown `(tract, event)` rather than reject it**; the registry is documentation beside the schema, never a constraint the schema enforces.

⚠ Drift, measured against that registry rather than assumed. The earlier three-way framing holds, but the numbers do not survive resolving dynamic dispatch:
- **produced but undeclared — 21 pairs**, including **three whole tracts the registry does not know exist**: `storage` (`think/supervisor.py:1668`), `support` (`apps/support/events.py:80`) and **`link`** (`apps/network/routes.py:871`, `convey/secure_listener/runtime.py:83-89`). 🔴 `link` matters most: `scripts/check_spl_health_vocabulary.py:136-142` calls its health event name **"a hard contract"** enforced Python↔Rust, and it is in neither the registry nor `docs/CALLOSUM.md`.
- **emitted and prose-documented but absent — 6 pairs.** ⚠ The prose drifts the other way too: `docs/CALLOSUM.md` documents 10 tracts and **omits `chat` entirely**, the registry's largest (16 events) and the one the root SSE operation names.
- **declared with no producer — 2, not 42.** A literal diff says 42; resolving the three module-level wrappers (`convey/chat_stream.py:403`, `think/thinking.py:895-898`, `think/cortex.py:1220`) collapses all but **`sync.status`** — no producer anywhere, and `docs/CALLOSUM.md:126-130` names a source file, `observe/sync.py`, **that does not exist** — and **`cortex.info`**, which is only ever written to the talent run log (`cortex.py:1291-1297`), never to the bus. 📌 That second one is a contract-shape finding, not bookkeeping: the registry declares a bus event that exists only in the durable half.

✅ **RECONCILED 2026-08-08 — the drift is CLOSED and the registry now matches emission.** The registry went **11 tracts / 57 concrete pairs → 14 / 80**; produced-but-undeclared is **0** and absent tracts are **0**, measured by a re-runnable instrument rather than by hand. ⛔ **`sync.status` remains declared on purpose** — it has no producer in any language, but a complete owner-facing UI hangs off it, and the declaration is the only breadcrumb pointing there. It is removed only when that UI is (operator decision, 2026-08-08).

⚠ **A FOURTH dynamic wrapper exists and it is a different shape from the other three: an *injected callable*.** The secure listener takes a `callosum_emit` in its constructor, stores it as `self._emit` defaulting to a no-op, and the runtime wires it to the `link` tract — so every `self._emit("<name>", …)` there is a `link.<name>` producer. 📌 **A scan can see the call but not the wiring**, so an injected wrapper is invisible to one built for module-level functions. This was found the hard way: the reconciliation declared two `link` events that the measuring instrument then reported as producerless. **The wave was right and the instrument was wrong.**

✅ **RE-MEASURED INDEPENDENTLY 2026-08-07 and the pre-reconciliation numbers held exactly** — 21 undeclared, 2 declared-with-no-bus-producer, 3 absent tracts — by an AST scan of the production tree (`build/lib/` and `tests/` excluded) with the then-known three dynamic wrappers resolved by hand. `cortex.info`'s durable-only status re-confirmed on the tree: it is written from the `JSONDecodeError` branch, which never reaches the bus emit. Three facts the earlier count did not carry:

- 🔴 **Six of the 21 undeclared pairs are the `supervisor` tract itself** — `request`, `restart`, `drain`, `skipped`, `sync_conflict`, and the registry declares only `started`/`stopped`/`restarting`/`status`/`queue`. **The journal's most-used control surface is absent from the contract that documents it**, and it is `P-system`'s own.
- ✅ **The `chat` tract IS enumerable, unlike the rest.** `chat_stream.py`'s `_VALID_KINDS` is a closed dict that **raises** on an unknown kind, so every declared `chat` event has a producer by construction. ⛔ That is the one tract where a rewrite may rely on a closed set — and it is a property of the *producer*, never of the registry.
- ⚠ **Part of the `cortex` overhang comes from talent execution, not from cortex.** The producer paths write `text_delta` and `tool_budget_exhausted` into the talent run log, which `cortex.py` re-emits verbatim. A scan that stops at `cortex.py` and `talents.py` under-counts.

⚠ **And the published schema requires a field the envelope makes optional.** `CallosumEvent` declares `"required": ["tract", "event", "ts"]`, while the server stamps `ts` only when absent. Today that is consistent because a Python peer always reaches the server first — but a producer that emits without a stamping server in front of it puts a document on the wire that the published schema rejects.

📌 **`P-segment-sense` produces on three tracts only** — `observe` (`detected`, `observed`, `status`, `described`, `transcribed`, `memory_throttle_started`, `memory_throttle_completed`), `notification` (`show`), and `supervisor` (`request`, from `observe/transfer.py:437`). ⛔ **`observe.observing` is NOT this plate's** — it is the ingest side's trigger *into* it (`apps/observer/routes.py:1289`, `think/importers/cli.py:1243`).

### `S:segment-sense:journal-segment-events`
**Connects** `P-segment-sense` → `P-journal` · **Owner** `P-journal` · **Tier** fixture

The **durable** half of the callosum contract, split from the wire half above. Bus envelopes are appended verbatim into `{day}/[{stream}/]{segment}/events.jsonl`.

⚠ **The bus writer bypasses `journal_io`** — bare `open(…, "a")`, failures swallowed at `logging.debug` — and 🔴 **it is not in this plate at all: it is `think/supervisor.py:6034-6068`**, wired as a callosum client callback (`:6763-6764` → `:6132-6141`). A rebuild looking for it inside sense will not find it. It filters to tracts `observe`, `think`, `activity` (`:6040`), requires `day` and `segment` (`:6046`), and **silently drops a well-formed event whose segment directory does not exist yet** (`:6058-6059`) with no log line at any level. The `journal_io` helper it bypasses (`think/journal_io/append.py:12-37`) is the one that fsyncs per record and fsyncs the parent on create, and seven other modules use it — this is an outlier, not an era.

🔴 **CORRECTED 2026-08-05 — three things this entry said, measured against the tree:**

1. **There are TWO writers with OPPOSITE discipline, appending to the same file.** The second is Rust: `core/crates/solstone-core-segment/src/sidecars.rs:9-13` goes **through** `solstone_core_journal_io::append_jsonl` and **propagates** — its caller turns a failure into a `500` (`solstone-core-ingest/src/router.rs:511-518`). ⛔ The one write discipline this entry asks for is not merely absent; the tree already contradicts itself on it.
2. **The file is NOT one shape, in two separate ways.** ⚠ *Within* the bus family the key set already varies per emission — measured on a real journal, one segment's 33 rows are all `think.status` and carry **two different key sets**, because the envelope is `{**defaults, tract, event, **fields}` and `fields` is whatever the emit site passed. ⛔ So the durable model is open and preserving, never a fixed struct. *Across* families, the Rust ingest path appends `DeviceIngestEvent` (`solstone-core-ingest/src/model.rs:155-167`) — `record_type`/`record_version`/`outcome`/`did`/`source`/`files`, and **no `tract`, `event` or `ts`**. The Rust reader discriminates on `record_type` (`solstone-core-ingest/src/events.rs:21-23`); the Python readers discriminate on `tract` (`think/segment.py:194-196`), so each family is invisible to the other's readers. ⚠ **Measured 2026-08-05: no journal contains both today** — the `DeviceIngestEvent` writer has no live caller, so the cross-family case arms when the native ingest path goes live, and it is a landmine rather than a live defect. ⛔ A rewrite that models this file as a list of bus envelopes still silently drops every ingest attribution record the moment it does.
3. **It is NOT append-only.** 🆕 🔴 **CORRECTED 2026-08-17 — the rewrite is native now.** `journal segment move` is in `NATIVE_PROCESS_SPECS`, and `core/crates/solstone-core-segment/src/relocate.rs` `rewrite_events` restamps `day`/`segment`, preserves unparseable lines byte-for-byte, and writes via `atomic_replace` — not a hand-rolled `events.tmp`. The crash-residue concern does not carry over. The earlier Python rewrite (`think/segment.py:380-409` via `events.tmp` + `os.rename`) is no longer the live path.

⚠ **Readers are 4, not 2**, and `apps/observer/utils.py:1242-1261` is not one of them — it is a *filename predicate* that never opens the file. The real readers: `think/segment.py:176-198` (tolerant — skips undecodable lines), `think/segment.py:499-503`, and **`solstone-core-ingest/src/events.rs:10-28`**, which is **strict on the same bytes** — any malformed line fails the whole read with `IngestEventLogMalformed`. 🔴 A torn line from the non-fsynced Python append is survivable to one reader and fatal to the other.

⚠ **No rotation, no size cap, no compaction.** Retention never names this file (`think/retention.py:588-604` unlinks raw media only) and `log_retention.py` scans no segment directory. It grows unbounded for the life of the segment.

### `S:segment-sense:thinking`
**Connects** `P-segment-sense` → `P-thinking` · **Owner** `P-thinking` · **Tier** schema

⚠ The talent event vocabulary is **NDJSON on stdout** — a real inter-process wire. Cogitate emits its provider events through the native one-shot wire, validated against `core/fixtures/cogitate_wire_contract.json`; Cortex still owns the subprocess lifecycle and relays the parsed records onto Callosum.

🆕 ⛔ **CORRECTED 2026-08-10; UPDATED 2026-08-11 — "no schema, no fixture and no validator anywhere in the tree" was FALSE, and it was quoted into two wave scopes before anyone checked it.** `core/fixtures/callosum_registry.json` is a **committed fixture read by a Rust crate** (`solstone-core-callosum/src/registry.rs`), and its `cortex` entry now enumerates **16 kinds**: `request · start · thinking · tool_start · tool_end · finish · error · talent_updated · info · status · cancel · dry_run · progress · text_delta · tool_budget_exhausted · budget_escalation`. The retired Python SDK subprocess runtime was the sole producer of the removed kind. `solstone-core-cogitate-wire` owns cogitate's native producer, its per-kind schema fixture, and `validate_event`. ⛔ **A rebuild must derive from that fixture or pin against it, failing in both directions** — publishing a second enumeration beside it creates two disagreeing lists of one vocabulary, which is the class this document exists to stop.

⚠ **The registry remains wider than the native cogitate producer.** The lifecycle and generate paths also use this Callosum tract, while native cogitate emits only its mapped, validated subset. `talent_updated` remains a declared consumer vocabulary without a current producer; `dry_run` is cogitate-reachable before the cogitate branch; and `progress` is generate-only. 📌 The contract must preserve those ownership distinctions instead of inferring all event shapes from any one producer. For contrast, the sibling vocabulary on the same bus *is* runtime-validated: `convey/chat_stream.py:137-138` raises on an unknown kind and `:375-386` enforces per-kind required fields.

⚠ **`info` is not the only consumer-synthesised kind.** `error` is **also** synthesised by the consumer at five call sites — including the silent-death path — and those synthesised errors **do not set `terminal`**, leaning on the very default that makes absence lethal. And **`start` is transport-only**: the talent lifecycle emits it and provider `start` events are explicitly dropped. ⛔ A schema marking only `info` as consumer-side mis-describes two other kinds.

🔴 **CORRECTED 2026-08-05 — the wire and the persisted run log are NOT the same shape.** The producer-side sidecar writer is **dead in production**: `JSONEventWriter` accepts a path (`think/talents.py:142-152`) and the only production construction is `JSONEventWriter(None)` (`:2107`). The durable run log — `{journal}/talents/{name}/{use_id}_active.jsonl`, renamed to `{use_id}.jsonl` on completion — is written by the **consumer**, `think/cortex.py:707-733`, from what it parsed. It does not round-trip: cortex adds `use_id`, `name` and `day` (`:1200-1211`) and **synthesizes `info` records no talent ever emitted** — any non-JSON line becomes `{"event": "info", …}` at `:1289-1297` rather than an error. ⛔ Reading the run log as a recording of the wire overstates it in both directions.

⚠ **The legacy Python TypedDicts are not the current cogitate contract.** Native one-shot events are validated by their fixture; Cortex's lifecycle events retain their own relay and consumer semantics. The remaining `Event` union does not describe every Callosum record (for example, `tool_budget_exhausted` and consumer-synthesized `info`), so consumers must keep treating the registry and per-producer schema as separate contracts.

⚠ **Every event name on this wire becomes a `cortex.*` bus event by variable** — `cortex.py:1216-1224` pops `event` from the parsed line and relays it. So this strand's vocabulary is the hidden source of `S:segment-sense:system`'s largest grep-invisible drift class, and `cortex.unknown` is reachable from any talent line lacking an `event` key (`:1219`). ⚠ Terminal detection is by name — `finish`, or `error` whose `terminal` **defaults to `True` when absent** (`:1241-1244`), so a provider omitting an optional field kills the run.

🆕 **The conversion's shape here, decided 2026-08-09.** ✅ **This strand does not need a new seam — it IS the seam.** The producing side is a subprocess emitting NDJSON on stdout today, so converting `cogitate` **replaces the producer** rather than inventing a boundary; the Python that survives is a thin client that spawns `solstone-core cogitate` and relays, the same shape `models.generate*` took. the native producer ships that native producer's published schema, per-kind required fields, and validator; Python replacement is the later relay swap. ⛔ **The run log stays a separate, declared format** and is not merged with the wire. ⚠ **Two consumer-side defaults are contract decisions rather than implementation details and get written down rather than inherited:** `terminal` defaulting to `True` when absent, and a line lacking `event` becoming `cortex.unknown`. The native producer always sets both; the *consumer's* posture is the part nothing states.

### `S:thinking:journal-thinking`
**Connects** `P-thinking` → `P-journal` · **Owner** `P-journal` · **Tier** fixture

Talent output landing durably. Carries the failure semantics: retries, back-offs, days being complete, segments being complete.

**Carry forward:** the talent-use lifecycle where **the filename is the lock and the state** — exclusive `open(…, "x")` on `{use_id}_active.jsonl` is the claim, rename on completion, and on restart every leftover `_active` is terminalized with an error so an interrupted talent is never indeterminate.

🔴 **`_active.jsonl` is not only a talent convention — it is a deletion gate in two other subsystems.** Seven production readers, including `think/log_retention.py:368` (skips pruning them) and **`think/retention.py:199` (treats presence as "segment incomplete, do not purge raw media")**. A cleaner claim filename is a format change on the writer side and a **silent capability loss on the deletion side.**

### `S:thinking:local`
**Connects** `P-thinking` → `P-local` · **Owner** `P-local` · **Tier** schema + fixture

The local model lane. ⚠ Not a types boundary — it is loopback HTTP **plus a durable record**. See
[`plates.md`](plates.md) § `P-local` for what must and must not be carried across it.

🔴 **The record that crosses this strand is the local lane's own, not a `generate` response.** The
obvious design — have the local side emit the boundary's tagged-union response directly — is both
unsatisfiable and unobservable, and both halves are worth writing down because the design reads as
correct:

- **Unsatisfiable.** A refusal needs `reason`, drawn from the boundary's closed refusal vocabulary, and
  the only reason-code→reason mapping in `core/fixtures/generate_contract.json` is its conformance
  vectors — **every one of which is an exception raised above this strand.** No local reason code has a
  vector, so "read it from the fixture, hold no copy" cannot be satisfied for any refusal this lane
  produces.
- **Unobservable.** The refusal mapping already lives at the wire, which computes `retryable` and
  `blocking` from the fixture by reason code. A local side that also emitted them would have its answer
  **re-derived at the wire**, which silently wins. An implementation that hard-coded both booleans would
  produce byte-identical output.

  ⚠ **Updated 2026-08-09.** This used to say the wire resolves the reason *from the raising exception's
  class*, and that it round-trips through a Python exception. Both were true of the Python wire, which
  is gone: the wire is Rust and each provider arm classifies its own failures. 📌 **The conclusion did
  not move and its reason got stronger** — one derivation, at the end that owns the contract.

✅ **So this strand carries a local result record** — a completion with its usage, finish reason, budgets
and inference block, or a failure carrying a reason code and a detail — and the boundary above keeps
owning the contract translation. One derivation, at the end that owns it. The record's shape is pinned
in `core/fixtures/local_contract.json`.

⚠ **`finish_reason` is load-bearing across this strand.** A completion the provider cut off arrives as a
success, and the caller is the one that must notice; it is also normalised on the way through, so a
consumer matching the endpoint's own spelling sees nothing wrong.

⚠ **Bundled-local cogitate inference telemetry is part of this strand's durable surface, not a log.**
It writes one row per run — success and every error path — carrying the queue wait, the admission slot,
the serving capacity and its source, the prompt-cache state, the server timings, the retry index, the
outcome and the reason code. BYO and confidential local variants do not write this telemetry.

🆕 🔴 **CORRECTED 2026-08-09 — "one row per call" is FALSE on the `generate` path, and has been since
the cut.** Measured three ways. **(1)** The only writer of `health/local-inference/YYYYMMDD.jsonl`
anywhere in the tree is `record_local_inference` (`providers/local_admission.py:365`), called from
**exactly one site** — inside `run_cogitate`'s `finally`. **(2)** No Rust crate writes it: grepping
all of `core/crates/` finds `health/local-inference` in `solstone-core-local/src/admission.rs` (the
*admission* directory, a different path) and in `solstone-core-retention/src/logs.rs` (a **pruner**).
**(3)** Git bears out how: the bundled-generate cutover said in its own commit message that it
*"retains Python ownership of local-inference telemetry through
`local_admission.record_local_inference`"*, and the commit that cut the Python generate
implementation removed that call with nothing replacing it. **So the file records `cogitate` runs
only, while the overwhelming majority of local inference is `generate`.** ✅ **Not owner-visible —
the file has zero readers**, in either language; only retention touches it. ⚠ **But `cogitate` is
its LAST writer.** ✅ **Disposition 2026-08-10: retain the artifact family, owned by `P-local`.**
Do not restore a `generate` writer and do not retire the file. Cogitate remains the sole writer;
the inference block inside the result record is what the boundary above reads. 📌 The
honest reading is that its function already moved: this strand carries the inference block **inside
the result record**, which is where the boundary above reads it from.
🔴 And it is load-bearing for the boundary above in a way nothing states: the wire reports a hint applied
**only if the result carries an inference block**, so an implementation that drops the block makes the
applied-hints set empty forever, and an assertion that a hint is reported as not-applied is satisfied by
reporting nothing at all.

### `S:journal:index`
**Connects** `P-journal` → `P-index` · **Owner** `P-index` · **Tier** fixture

`journal indexer` is native end to end. The Python feature entry points terminate at Rust operations: backup restore and ordinary scan callers use native scan; chat, importer, day-accumulator, and segment append paths use native file rescan; observer prune and share delete use native path/stream pruning; entity merge uses native fold/rebuild/fingerprint operations; and `apps/search/maint/003_migrate_index_stream.py` invokes a native reset plus full rebuild. The Python implementations remain as reference oracles. The [real-index poison test](../../core/crates/solstone-core-journal-bin/tests/journal_identity.rs) fails Python launchers with exit 97 and verifies the resulting SQLite content.

`think/segment.py` also reads segment presence and chunk counts through an explicit `mode=ro` connection. It is a reader, not an additional mutation entry point.

⚠ Plus an **invisible runtime dependency** on `apps/speakers/edges.py` and the entity store via the `EDGE_SOURCES` registry — real, and invisible to any import graph. ⛔ **CORRECTED 2026-08-05: the index build does NOT run `find_matching_entity`.** `edges.py` imports exactly one entity symbol (`load_all_journal_entities`) and has zero references to `find_matching_entity` or `think.entities.matching`. Its name-variant matching is an **independent reimplementation** with its own process-global cache, and it stats entity files directly. ⚠ The previous wording asserted two matchers were one, which is exactly the merge a future scope would make on this document's authority.

🔴 **Three independent name-variant matchers exist** — `think.entities.matching`, `apps/speakers/edges.py`, and `entity_name_matcher.rs` — and under rule 1 only one may own the contract. **The name-variant matching contract belongs to `P-entity`**, the identity-bearing store and the one-to-many end that every matcher's consumers resolve against. ⛔ **Consolidating them is not in flight and is not required of any current lane** — this is recorded so the class stops being representable, not as work to start. ⚠ A wrong identity decision here re-partitions an owner's people silently.

**Carry forward:** the index is **always rebuildable** — ephemeral by design, and an interrupted update never leaves a partial result. ⚠ That property is required, not incidental.

### `S:index:format` · `S:web:format`
**Owner** `P-format` in both · **Tier** schema

The indexer's and the convey apps' consumption of consistently formatted structured journal data. ⚠ `P-format` owns both because it is the one-to-many end — it serves all consumers and cannot negotiate per-consumer. ⛔ The name encodes contract ownership, not data flow.

🔴 **`S:index:format` is built; `S:web:format` now has its formatting boundary, while its HTTP-route/convey-app serving surface remains unbuilt.** The rendered value is index-shaped (`chunks: [{content}]`), and the contract also carries a document **header**, a per-chunk **occurrence time**, and the **originating record**. The index stores none of those three, which is why their absence went unnoticed — and they are precisely what the owner-facing surface reads.

**Carry forward — what the web half needs, and why each exists:**

- 🔴 **The originating record is load-bearing, not decoration.** Speaker attribution reads it and the speaker is *stripped* from the rendered text so the surface can draw a structured speaker element instead; a 1-based sentence ordinal is assigned by matching chunks back to their source rows, and that ordinal is the key a speaker correction writes against. Screen frames read frame identity and bounding boxes from it, including per-participant boxes. Browser rows read it to tell a snapshot from a delta.
- 🔴 **The occurrence time drives audio seek**, not just ordering: clicking a transcript line converts it to an offset into the segment's audio, and playback re-derives the active line from it. Losing it does not degrade the timeline, it removes the interaction.
- ⚠ **The header is consumed by the text projections, never by the web route.** Do not infer it is unused from the route alone.
- ⚠ **Speaker labels resolve from journal config** — an owner display name, then an agent name, each with a fallback. ⛔ A renderer that hardcodes them disagrees with the owner's own journal on every turn; the formatting layer should take them as an **input** so it stays pure and the caller supplies them.
- ⚠ **Several consumers reach the formatters by direct import, bypassing the registry entirely** — the transcripts route, the timeline route, the activities route, and the text-projection wrappers. Two of those cannot go through the registry by construction: one formats an in-memory record that has no file at all, and one feeds entries synthesized from sidecars that were never on disk. ⛔ **A path-keyed registry cannot express either**, so the boundary needs a by-shape entry point that does not require a path.
- ⚠ **The projection walker is an iterator with a pre-render filter, not a map.** The filter runs *before* rendering; collapsing it to a name→text map moves that cost onto every caller.
- ⚠ **Talent-projection keys are owner-visible** — each becomes a tab label. Renaming a key renames a tab.

### `S:index:*` — the read / query path
**Owner** `P-index` · **Tier** schema

Production readers span search, tools, voice, talents and entity context, including edge readers in `apps/home/connections.py`, `think/curation.py`, and `apps/entities/routes.py`. This section tracks the `search_journal` / `search_counts` interface until it receives a dedicated strand. Both calls cross into `solstone-core-indexer-query`; Python shapes the returned JSON for still-Python consumers but does not open or count the index.

⚠ **The talent still asks twice.** `think/tools/search.py` calls both native search and native counts for one invocation, so it still pays for two process and query boundaries even though counts are no longer materialized in Python. That is an API-shape cost, not a Python SQLite read path.

⛔ **`known_agents` is NOT on the talent path** — do not group it with the two above. `think/tools/call.py` returns early into `think/tools/search.py` when JSON output is requested, so `known_agents()` is reached only from the human CLI with an explicit `--agent`. Its cost is an owner-CLI cost, not a per-talent one. ⚠ It is still a full scan of the chunk table to list a set whose measured cardinality is 31.

### `S:*:P-entity` · `S:*:P-facet`
**Owner** `P-entity` · `P-facet` · **Tier** fixture

See [`plates.md`](plates.md) — this store fails by **bricking**, not degrading.

### `S:*:journal-config`
**Owner** `P-journal-config` · **Tier** fixture

The many-to-one shape is the whole reason the contract sits here: **55 production modules** touch this
file — 31 through the reader, 19 through the mutator across 46 call sites — and the plate cannot
negotiate a posture per caller.

See [`plates.md`](plates.md) § `P-journal-config` for the fail-closed posture that is the house style,
the four readers that used to break it, the two default sets, and what the two *write new, read old*
shapes were measured to do.

🔴 **What every consumer of this strand is entitled to, and what it must not do:**

- **A missing config is not an error** — it yields defaults and the consumer gets values. **A config
  that exists and cannot be read or parsed is an error**, and the consumer carries it rather than
  substituting. ⛔ *"Log a warning and use a default"* is the failure this contract exists to prevent,
  and it is what three of the four broken readers do.
- ⛔ **`Path.exists()` is not the missing-versus-present test.** It answers `false` on *any* stat
  error, so an unreadable parent directory reads as *no config at all* and every reader silently
  substitutes defaults for the owner's real settings. Only a read failing with **not-found** is
  missing; every other read failure is an error. ⚠ The same trap catches an instrument — a symlink
  loop and a directory-in-place both report "absent" through `exists()`.
- ⛔ **The defaults are not a caller parameter.** A consumer never supplies a default set; the plate
  owns both of them. Two writers supplying different defaults is how a three-key set became the
  materialized contents of a config.
- ✅ **A consumer that only reads depends on the read half, and it now has one.** The durable-write
  primitives are banned outside the write authorities, and that ban is correct — but it is also why
  three crates once hand-rolled their own reader and two lost the posture. 📌 **A contract with no
  home a reader may legitimately depend on has no enforcement at the boundary it names**, and that
  generalizes past this strand: wherever a ban is right, check that the permitted side has somewhere
  to go.

### `S:web:thinking` — chat
<!-- strand resolved by deletion, not conversion (2026-08-20) -->
**Owner** `P-thinking` · **Tier** schema

The primary owner-facing use of the model. `convey/chat.py` (2,532 lines) + `chat_stream.py` (512) + `convey/sol_initiated/` (1,034). It **spawns talents**, so it is a producer into `P-thinking`, and it is in the audited native-client bundle — the capture clients depend on it.

⚠ **Push cannot be scoped until this exists** — push's trigger is the **chat tract** (`push/triggers.py:63-64`), so push is a callosum consumer downstream of the chat orchestrator, not a standalone journal→device path.
<!-- historical; push paused, chat trigger retired — future payload is journal state / device check-in -->

### `S:*:system` — the command channel
**Owner** `P-system` · **Tier** schema

`_handle_task_request` takes `message["cmd"]` off the unix socket and hands the argv to the task queue. **This is how the scheduler, importers, backup, and the sense/think pipeline all cause work to run.** ⚠ Distinct from liveness/status.

⚠ **CORRECTED 2026-08-07 by an AST scan of the production tree with `build/lib/` excluded — it is not six producers. There are 14 bus emit sites across 11 modules, and the bus is not the only way in:** eight direct `_task_queue.submit` call sites live inside the supervisor itself and **six of those never touch the bus at all.** The *verb* vocabulary is seven — `indexer` · `think` · `brain` · `importer` · `heartbeat` · `maintenance` · `facet-candidates` — and that number survives either count. The *argument* vocabulary does not: three argument forms appear only on the in-process path. 📌 **A census taken from the bus alone reports the right verbs and the wrong grammar**, which is the shape that matters for a typed rewrite.

**Carry forward:** the channel is modelled as unbounded argv and used as a bounded vocabulary; every production argv has head `journal` or `sol`. The single open door is the schedule config, whose `cmd` array is executed verbatim. ⛔ A rewrite that types the channel keeps that door as one explicitly-named variant reachable only from that config — closing it stops work the owner has configured, and generalizing it puts every caller back on unbounded argv.

✅ **CONVERTED 2026-08-10 — the channel is native**, in `solstone-core-system` (`request` · `partition` · `cap` · `queue`). The rewrite kept both halves of the carry-forward: the typed shape **and** the single named escape reachable only from the schedule config. ⚠ **The 14-vs-6 correction above is why it works** — a channel typed from the bus census alone would have carried the right seven verbs and the wrong argument grammar, silently dropping `--live`, `--stream`, `--expected-fingerprint`, and the `--segment`+`--flush` pair that only the in-process callers produce. 📌 **The reusable lesson for any strand of this shape: count the callers, not the emitters.** A bus census answers a question about the bus, not about the vocabulary.

### `S:journal:establish`
**Owner** `P-device-link` · **Tier** fixture

⚠ **Owner assigned 2026-08-05.** What establishment produces is the **identity root** — the promoted CA and the persisted instance identity — and `P-device-link` is the plate that owns identity and must serve every device that later pairs against that root. One-to-many end, so it owns the contract.

First-run journal establishment. **Creates the identity root** that `S:device-link:journal` depends on: the mark-lock route promotes the staged CA and persists the instance identity.

⚠ Eight `/init/*` routes are session-gate-exempt and admitted **before** the journal-is-active check. That is the same `localhost:5015` human-entry basis as everything else on `P-web`, before a session exists to gate — **not** a third access path.

### `S:journal-segment:peer-exchange`
**Connects** `P-journal` → `P-peer-exchange` · **Owner** `P-peer-exchange` · **Tier** fixture

⚠ **Owner assigned 2026-08-19, alongside adopting `P-peer-exchange` as a plate covering `transfer` and `export`.** The **archive manifest v1** — the durable format `transfer export` writes and `transfer import` reads: segments, sha256 + size per file. Cross-instance — the far end may be running a different journal version — which is the one-to-many shape rule 1 puts the contract at the receiving end for.

✅ **Published 2026-08-19** — `schema/archive-manifest.v1.schema.json` in `solstone-core-transfer`, JSON Schema draft 2020-12 with an `x-journal-contract` block and hand-verified examples validated by a committed test. Publishes the contract only; no runtime validation is wired into `transfer export`/`transfer import`.

### `S:device-link:peer-exchange`
**Connects** `P-device-link` → `P-peer-exchange` · **Owner** `P-peer-exchange` · **Tier** schema

⚠ **Owner assigned 2026-08-19, alongside adopting `P-peer-exchange` as a plate.** The **peer-ingest HTTP surface**, `/app/import/journal/{prefix}/…` — six operations across five areas (`config` · `entities` · `facets` · `imports` · `segments`, each a POST, plus one GET `manifest/{area}`). Rides the paired-peer transport `S:device-link:journal` already authenticates.

🔴 **CORRECTED 2026-08-19 — this entry originally said "nine operations."** That figure was inherited unverified from a lane workspace and appears to have been conflated with `P-device-ingest`'s separately-cited *9 published operations* — a different plate. Re-counted directly against `solstone-core-import-web/src/lib.rs`'s route table (6 routes under this prefix) and cross-checked against the published contract below.

✅ **Published 2026-08-19** — `schema/peer-ingest.v1.schema.json` in `solstone-core-import-web`, JSON Schema draft 2020-12, one subschema per operation, with an `x-journal-contract` block and hand-verified examples validated by a committed test. Publishes the contract only; no runtime validation is wired into the HTTP path.

---

## Tier 1 — distribution

`P-distribution` connects through two strands. See [`plates.md`](plates.md) § `P-distribution` for the artifact layout and the carry-forward invariants.

| Strand | For | Owner | Tier |
|---|---|---|---|
| `S:*:distribution` | **what the artifact must contain** — every binary, shared library and data asset a plate needs on an owner's machine, and where in the tree it is placed | `P-distribution` | fixture |
| `S:distribution:system-models` | **the ship-versus-fetch boundary** — which artifacts travel inside the release and which the owner's machine downloads afterwards | `P-system-models` | schema |

🔴 **The first strand is the release inventory, and rule 1 puts the contract at `P-distribution`'s end** — it is the one-to-many end, it serves every plate that needs something on disk, and it cannot negotiate a different layout per plate. ⚠ **Today that contract exists only in fragments** — `scripts/release_package_inventory.py`, `scripts/check_wheel_contents.py`, and a `dependencies` list in each of thirteen `packages/*/pyproject.toml` — which is the two-places-own-one-thing class this rule makes unrepresentable rather than merely detectable. ✅ **Its home is a declarative file the producer reads**, on the `core/ci/*.toml` + `solstone-ci` pattern; ⛔ **the inventory is data, and the thing that reads it is the same Rust binary that emits every container**, so a plate's claim on the artifact cannot be satisfied in one format and missed in another.

⛔ **An inventory is an allow-list, never a directory listing.** A default release build produces test binaries and stub helpers alongside the product ones; see the carry-forward in `plates.md`.

⚠ **The second strand is where a name is decided rather than defaulted.** `P-system-models` already owns *what an owner's machine downloads, where it comes from, and whether the host can run it*, and it enforces a single-element origin allow-list inside one fetch primitive. ⛔ **Shipping an artifact and fetching it are both defensible; leaving the answer implicit is not** — that reproduces the nobody's-contract shape one layer down.

🔒 **RULED 2026-08-16 — a REQUIRED artifact is bundled.** An artifact without which the journal cannot perform a function every install has is carried **inside** the release tree, not fetched. The boundary this strand draws is *required versus optional*, and it is a property of the artifact rather than of its size.

| | | |
|---|---|---|
| **Bundled** | the three speaker/VAD graphs — `wespeaker-resnet34-256` · `pyannote-segmentation-3.0` · `silero_vad_v6` | every install needs all three, they are digest-pinned in the consuming crate (`solstone-core-transcribe/src/model_assets.rs`), and transcription is not an opt-in feature. Placed at `lib/solstone_journal_models/assets/`, which that crate's resolver already searches **before** any `site-packages` candidate |
| **Fetched** | the large per-platform optional runtimes — `llama-server`, the Core ML transcription models, ced, rf-detr, the Vulkan probe's targets | gated on host capability or on an owner choosing a provider; **not** every install needs them, and the mirror, catalog and deletion-guard machinery in `P-system-models` exists for exactly this class; ced is required parity that happens to be fetched |

⛔ **The fetched class does not grow by default.** Moving a required artifact out of the tree to save download size is a change to this contract, not a packaging optimization: it converts a working install into a first-run network dependency, and it is the strand's owner who decides it.

---

## Tier 1 — retention

`P-journal-retention` connects through five strands, each a different contract:

| Strand | For | Owner | Tier |
|---|---|---|---|
| `S:journal-retention:journal-config` | the **posture / settings** it reads | `P-journal-config` | fixture |
| `S:journal-retention:system` | **when it runs** | `P-system` | schema |
| `S:journal-retention:journal` | **tending the files** — changes, and recording status | `P-journal` | fixture |
| `S:journal-retention:index` | **telling the indexer what was removed**, after the removal | `P-index` | schema |
| 🆕 `S:*:journal-retention` | **asking retention to remove owner media** — the only way in | `P-journal-retention` | schema |

🔴 **The segment is the unit of deletion, retention executes every removal, and retention tells the indexer afterwards.** That ordering is the design: removal happens first and the index is informed, never the reverse. The fourth strand above exists because of it.

🆕 🔴 **The fifth strand is the removal request, and retention owns it** — minted 2026-08-05 by operator authorization. Retention is the **consumer** on the other four; every one of those contracts sits at the far end. This is the one relationship where it is the provider, and rule 1 puts the contract at retention's end because it serves all comers and cannot negotiate per-caller. ⚠ **Four plates request removals** — the owner's segment delete, `P-segment-sense`'s terminal-empty hand-off, the backup offload, and retention's own configured policy — and until this strand existed that contract had four callers and no name, which is the two-places-own-one-thing class rule 1 makes unrepresentable rather than merely detectable.

🔴 **A removal request carries its own precondition, because consolidating the removers must not weaken the strongest one.** The offload pass archives to backup, **confirms the snapshot holds every byte at the recorded size**, appends its ledger, and only then mints an approval-required removal mark — where retention's own path hashes the bytes and removes with no archive. When that removal becomes a request, the confirmed-snapshot precondition travels **with the request**. ⛔ A request type constructible without it has moved the guard out of the executor and into the caller.

⛔ **Two units, and the request names which.** Whole segments — one, or a set — leaving a `tombstone.json`; or the proven raw originals, leaving every derived output. ⛔ There is no third unit and no partial owner-directed delete. See [`plates.md`](plates.md) § `P-journal-retention`.

⚠ **`S:journal-retention:system` has two shapes that must not be conflated:** *when the policy sweep runs* — a schedule entry, `P-system`'s contract — and *a removal request arriving from another plate*, which is synchronous and needs no schedule. 🔴 **Today the first does not exist**: nothing schedules the raw-media pass, so two of the three configured retention modes never execute. See [`plates.md`](plates.md).

✅ **`S:journal-retention:index` now has its entry point.** `prune_by_paths` matches each removed path exactly **and** as a directory prefix, so passing a segment clears the segment and everything that was inside it; a path the index never held is ordinary rather than an error, and it deliberately does not create an index that does not exist. ⛔ The earlier note that this had "never been ported" is retired. ⚠ The stream-keyed prune still exists alongside it and is the wrong tool for a removal — a segment under one stream that held another source's originals keeps its rows if pruned by stream name.

🔴 **Why the ordering is a safety property and not a preference.** Index discovery is a pure filesystem glob with no database input, and the scan deletes any row the glob no longer produces — so the index re-converges on the chronicle every run. **Remove, then notify:** a crash between them leaves stale rows the next scan deletes, because the file is gone; self-healing, and the owner is never told something is gone that is not. **Notify, then remove:** a crash leaves the index missing rows for a file *still on disk*, so in that window the owner has been told their data is out of search while it is still there — and the next scan **puts it back**. Only one order is safe from the owner's point of view.

Where retention is the provider it owns the contract — it is the one-to-many end, reached from **11 import sites across 7 production modules**. ⚠ Those 11 are not 11 decisions: one is the purge itself, two consult the deletion gate, and the rest read storage totals, the config, or a byte formatter. **Count the callers of the irreversible verb, not of the module.** See [`plates.md`](plates.md) for the irreversible-deletion carry-forward and the measured predicate.

---

## Tier 2

| Strand | Owner | Note |
|---|---|---|
| `S:system:system-health` | `P-system-health` | Liveness and current status of running things. ✅ **Native 2026-08-10.** `stale_heartbeats` is no longer an empty publication — it is derived from the sync check's non-live foreign writers and **fails closed** |
| `S:system-health:journal-health` | `P-journal` | 🔴 The per-day health JSONL grammar is **18 record kinds**, of which the reader consumes 10. ⚠ Mostly Python string literals, but **three sets already have owners** and re-typing them forks them across languages; the **mode set has no owner anywhere**. ⛔ The writer belongs to `P-thinking` and the contract to `P-journal` — a schema binds three plates and is not `P-system-health`'s to change alone |
| `S:web:journal` | `P-journal` | ⛔ 100% of `P-web` is `localhost:5015` or an authorized linked device. ✅ **The localhost half runs natively (2026-08-10).** `journal convey` enters `solstone/convey/cli.py` and `execv`s `solstone-core convey`, which binds the loopback listener and serves the shell, app registry, session gate, and speakers surface with no Python in the request path. Speakers is converted; the other 20 app surfaces return the named 501 refusal. ✅ **THE LINKED-DEVICE HALF IS NOW NATIVE TOO (2026-08-11), and the two halves are ONE surface.** `solstone-core convey` binds `0.0.0.0:7657` alongside loopback in the same process and serves linked devices over TLS 1.3 mutual auth against the journal’s own CA, then the SPL mux, then one HTTP/1 exchange per peer-opened stream — through **the same `Router` value** the loopback listener serves, so the halves can no longer diverge. Proven by execution, not by tests: a device on a second machine reached the journal over the LAN door with HTTP 200 and a populated shell payload, one process holding the door and both loopback sockets; a wrong CA is refused by fingerprint pin and an unauthorized client certificate by a TLS `CertificateUnknown` alert. ⚠ The **relay** leg terminates at that same socket (`journal spl` dials `127.0.0.1:7657`), so one dark port took both legs down — which is why the outage presented as a TLS timeout rather than a refusal. ⚠ `ViaSpl` carrier attribution is not yet proven live. ⛔ The authorized breakage is discharged. 🔒 **Repaired as the linked half of this strand converting**, so `P-device-link`’s authorization boundary moved with it rather than staying in Python — superseding this note’s earlier “not `P-web`’s to repair … Python by ruling” clause, which the 2026-08-09 replace-convey-outright ruling had already overtaken. 📌 Recorded because a boundary that has quietly stopped working, with its code still on disk reading as live, is how a security surface rots |
| `S:cli-journal:journal` | `P-journal` | Same-device only; `solstone-core-journal` holds the local capability and its Rust archive/facet/news authorities may modify the journal directly |
| `S:cli-sol:web` | `P-web` | Any device; `solstone-core-sol` has API/link transport and no journal-local dependency or identity switch. ⚠ May share `S:web:journal`'s contract |
| `S:segment-sense:speaker-id` | `P-speaker-id` | A refinement of sense |
| `S:speaker-id:journal-facet` | `P-journal` | ⚠ **Years of voiceprints must survive with no re-teach** — a shipped promise. ✅ **Keepable at the read layer, proven 2026-08-07 by running the shipped native reader in place against a real journal: 44/44 files, 51,186/51,186 rows, 0 failures** (⚠ read half only; not writer round-trip, not resolution equivalence, N=1). **One metadata keyset, no variation:** `added_at`·`day`·`last_seen_ts`·`segment_key`·`sentence_id`·`source`·`stream`; embeddings `<f4` (N,256). 🔴 **The format carries no version and no encoder identity while the reader hard-codes width 256 — an encoder change IS a forced re-teach**; the conversion adds an additive `envelope.npy` and relaxes the reader to a required-subset, absence meaning legacy and never corrupt. **Carry forward:** the label-source resolver returns *ambiguous* and warns the owner rather than picking a source (⚠ its `.npz` branch does pick `first()` among several — deterministic, but not the same posture). ✅ **CONVERTED 2026-08-09 — write ownership is now `core/crates/solstone-core-speaker-resolve/`, reached via `solstone-core speaker-resolve <verb>`, with `solstone/apps/speakers/speaker_resolve_transport.py` the SOLE Python transport.** The no-re-teach promise was re-proven **after** the cut, in place, unchanged: **44/44 files, 51,186/51,186 rows.** ⛔ **Read-compat is now permanent, not transitional** — the store holds v0 envelopes written by a Python writer that no longer exists, so the tolerant reader can never be tightened to require v1. ⚠ **The Python symbols survive as transport shims and as the entity-merge writers** (`think/entities/merge.py` only), so ⛔ **never gate this strand on symbol existence — gate on whether a call resolves a write to a speaker artifact path.** ⚠ **No cross-language differential exists for this strand**; the Python oracle is recoverable at `45990f652`. 🔴 **Historical pre-migration warning — now closed:** the manual/UI contamination guard was not *"the fail-open version"* of its automatic Rust twin: it was a **superset guard with a fail-open tail**, screening against a **provisional centroid** derived from the owner's manual tags in exactly the window where the automatic path refused to write at all. ⛔ **A native screen built only on `load_owner_centroid` would have closed the tail AND deleted that tier** — taking a guard away during bootstrap, when the owner is teaching the system who they are. **The third hole was not in any handoff:** `load_owner_centroid` swallows every exception and the provisional loader refuses on file **presence** rather than on a successful load, so a **corrupt** centroid makes the guard allow every write — an owner who has fully taught the system loses it entirely, with a log warning as the only signal. ✅ **The provisional tier is native as of `1ca554d22`** (`owner_provisional.rs`), resolving a tier plus a **ten-variant reason**; ⚠ its central hazard is that the shipped `load_owner_centroid` returns `Ok(None)` for **absent** *and* for **zero-norm**, so the file's existence must be consulted **separately** or a provisional centroid resolves where the reference resolves none. ✅ **The *ambiguous-rather-than-pick* posture this strand already carries now extends to the guard**: no resolved centroid yields `indeterminate`, which **refuses and carries the reason** — ⛔ never a `false`. ✅ **The native route routes the former Python UI calls through the native fail-closed screen.** |
| `S:body-source:journal-body` | `P-journal` | Owner **body** data: ingress, not egress. ✅ **CONVERTED 2026-08-09:** Rust owns the bounded normalized row/hash, immutable bundle envelope + ledger + retained-raw inventory, replay, and atomic dedupe rebuild. SQLite is derived and excluded; native bundles survive backup, retained raw is digest-verified, and restore rebuilds identical Apple+Oura dedupe state before success. Python remains only process transport and an independently exercised differential reader oracle. |
| `S:file-import:journal-segment` | `P-journal` | Generic import. ⚠ **A second write door into `chronicle/`** that consults no contract and writes with bare `write_bytes` — the create-exclusive guarantee is a property of **one** of the two doors |

## Tier 3 — additive, absent until approved in

`S:thinking:byo` — ⛔ egress, the owner's own key · `S:thinking:spp` — ⛔ egress, attested. ⚠ Fails closed by the *absence* of a fallback branch; make "no downgrade path" an explicitly tested invariant.

## Not yet placed

- **push** — ⚠ cannot be scoped until `S:web:thinking` exists; its trigger is the chat tract. Only reimplemented end-to-end encrypted.
<!-- historical; push paused, chat trigger retired — future payload is journal state / device check-in -->
- **encrypted backup** — blind by construction, but ⚠ **retention imports it**, so a core deletion path depends on it. 🔴 The 64-char recovery key **is** the repository password. **Carry forward:** the exclusion-list comment records a paid-for lesson about basename-at-any-depth matching that a rebuild would otherwise re-learn; and `brain.json` / `scheduler.json` / the supervisor-ready marker are excluded deliberately, so post-restore provider and scheduler state is an intentional blank.
- **support request** — ⛔ real egress; the `_SECRET_*` redaction is the last thing before an external service.
- **merge / transfer** — deferred. ⚠ `think/merge.py:30-33` imports three **private** functions out of `think/entities/merge.py`, so the frozen thing depends on the unstranded store.
