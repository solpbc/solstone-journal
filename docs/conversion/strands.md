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

Segments arriving from a device. This is the **ingest envelope** — a wire shape, not bytes on disk: `observe/protocol.schema.json` carries `file_kind: "ingest_envelope"` and `producer_write_paths` of the ingest endpoint.

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

⛔ **RETIRED 2026-08-05 by operator ruling — do not re-derive it.** This entry used to say that a new sidecar name silently widens what the owner's delete removes, because the delete classified a segment as *location-only* or *mixed* using `RESERVED_SEGMENT_FILENAMES` as its non-content set. **The classification is gone**: the segment is the unit of deletion, a segment is removed whole or not at all, and no name set decides anything about it. See [`plates.md`](plates.md) § `P-journal-retention`.

📌 **What the retirement is worth keeping.** The divergence was real and was measured before the rule changed: Python's set is 3 names (`think/segment_files.py`) against the segment crate's 6, which produced *different delete outcomes on the same journal* — `segments: 0 / mixed: 6` in Python against `segments: 3 / mixed: 3` in Rust. ⚠ The set still matters for what a client may not upload (`is_reserved_name`), and **`device.json` and `tombstone.json` are in neither Python's reserved set nor `is_structural_derived_file`**, so `apps/observer/prune.py` refuses any segment carrying one as a `derived-output` unknown. That arms at cutover. `events.jsonl` is fine — structural-derived covers it by name.

⚠ **`stream.json` is read far more widely than the plate's own notes suggest — 17 production call sites across 8 modules**, led by `think/segment.py` (6), `apps/settings/maint/*` (4) and `apps/observer/prune.py` (3), plus `think/cluster.py`, `think/streams.py`, `think/indexer/journal.py` and `observe/sense.py`. 📌 An earlier count of "six sites in `prune.py`" was counting error-message string literals, not readers. It matters because the marker carries `prev_day`/`prev_segment`/`seq`, and the reference writer **resets `seq` to 1 and stamps a fresh `created_at` whenever the stream record fails to parse** (`think/streams.py:214-232`, a swallowed `JSONDecodeError | OSError`) — forking the chain for all 17, silently, on a path that looks like a successful write.

🔴 **The content-identity reference refuses in three cases and silently degrades in the rest.** `_read_ingest_manifest` returns `{}` for *every* failure — unreadable, not JSON, non-dict root, unknown `schema_version`, non-dict `files`, or any entry that is not an object — and `content_identity_from_segment` reads that as falsy and **falls through to the legacy media-scan arm**, producing a valid identity computed from whatever is on disk. One of its own refusals is unreachable as a result. ⛔ **A rebuild must refuse instead**: exists-and-will-not-parse raises, missing returns defaults. The two refusals that genuinely protect owner media — on-disk bytes disagreeing with the manifest's `sha256`/`size`, and a manifest-named file absent without terminal processing proof — the reference does get right, and both must survive.

**Carry forward:** **unresolved error → hold raw**, via `exit 69` — the handler writes nothing, leaves the input, and the scanner records *neither* success nor failure. The deferral emitter deliberately swallows its own bus failure so a down bus cannot turn a deferral into data loss.

### `S:segment-sense:journal-segment`
**Connects** `P-segment-sense` → `P-journal` · **Owner** `P-journal` · **Tier** fixture

Processed sense output written back.

⚠ **The `_solstone_processing` header moved out of this strand 2026-08-05** — it is now `S:segment-sense:segment-processing`, below. What stays here is the analysis output itself: `<stem>.jsonl` rows and the `<stem>.npz` speaker-embedding sidecar, whose formats `observe/screen.schema.json` and `observe/transcribe/audio.schema.json` own.

⚠ Both of those schemas **under-declare their own headers**, and both carry `additionalProperties: true`, so nothing ever fails validation. Undeclared but written: `_solstone_thinking` (`describe.py:653-657`) and audio's `overlap_fraction`, `overlap_detector`, `device`, `compute_type`, `speaker_analysis_producer`, `noisy_rms`, `noisy_s`, `loud_windows`, `speech_loud_windows`, `loud_speech_ratio`.

**Carry forward:** **terminal-empty written before the raw is released** — die between them and the file survives with a terminal marker, never the reverse · atomic promote through a same-dir temp, header **last** · detections stored raw, filtered at read time.

⚠ **Three scope corrections to those three, all verified by reading the code:**
- 🔴 **The unlink half of terminal-empty is RETIRED 2026-08-05 by operator ruling — the marker discipline is not.** `transcribe` writes the terminal-empty marker exactly as it does today (`transcribe/main.py:1182-1191`, `:810-819`; durable because `write_text` fsyncs the temp, `os.replace`s, then fsyncs the parent) and then **hands the raw to `P-journal-retention` instead of calling `unlink()`** (`:1209`, `:836`). ⛔ Retention is the only plate that removes owner media. ⚠ The *ordering* invariant survives intact and still matters — the marker must be durable before the raw is handed over, never after. The discipline was transcribe-only in any case: describe writes the empty marker and never unlinked the video.
- **"Header last" is true of the *decision*, false of the *byte order*.** In the promoted file the header is physically line 1; what happens last is *determining* it, inside `_promote` (`describe.py:896-925`), once the run knows its verdict. ⛔ A rebuild that appends the header at the end of the file has read this backwards.
- **Detections are filtered at exactly one read site** — `qualified_objects` (`observe/detect.py:106-122`) has a single production caller, `observe/screen.py:225`. ⚠ So `depict`'s stored `source="still"` detections are **never** filtered on any read path.

### `S:segment-sense:segment-processing`
**Connects** `P-segment-sense` → `P-segment-processing` · **Owner** `P-segment-processing` · **Tier** fixture

🆕 **Added 2026-08-05 by operator ruling**, split out of `S:segment-sense:journal-segment`. The per-file outcome ledger — `_solstone_processing` — and the predicates every reader decides against. See [`plates.md`](plates.md) § `P-segment-processing` for why it is its own boundary.

🔴 **There are THREE producers, and the third is the one that matters most.** ⚠ This line previously read *"describe and transcribe only"* and then named a third in the same sentence — a self-contradiction that is exactly how the third gets omitted downstream.

| producer | writes | via |
|---|---|---|
| `observe/describe.py:915` | screen verdicts | `build_processing_record` (`observe/processing_record.py:160-190`) |
| `observe/transcribe/main.py` at **`:652`** (empty), **`:679`** (failed), **`:900`** (analyzed) | audio verdicts | same ⚠ `:611-612` is the metadata-assembly line, not a call site |
| 🔴 **`think/backfill_processing_records.py:170-198`** | `state=empty` with `source="backfill"`, and `input_size` from the media sibling — **or `0` when the sibling is absent or `stat()` raises** | same |

🔴 **The third stamps a verdict no handler produced, and both terminal-proof implementations ignore `source` entirely.** So an operator-stamped `empty` licenses retention to purge and licenses a device to drop its only copy, with nothing having processed the file. ⚠ Measured: a real journal's screen outputs that carry **no record at all** — and therefore cannot grant proof — are precisely this tool's target population, so one operator command converts them from proof-less to proof-bearing. ⛔ `source` is the record's only provenance field; a rebuild must carry it, and must not quietly resolve this.

⚠ **`observe/depict.py` writes no record at all**, so image outputs are invisible to every reader — and both terminal-proof implementations refuse any extension outside audio/video anyway (`apps/observer/processing_proof.py:26-33`, `terminal_proof.rs:71-77`), so **an ingested image can never be proven consumed** and the sending device never releases its local copy. 🆕 **Operator ruling 2026-08-05: `depict` is promoted to first class** — it gains a record, a schema, re-entry and a formatter entry, and this hole closes.

**The 9 production read sites, measured** — ⚠ "read by five planes" was a floor:

| Reader | Decides |
|---|---|
| `observe/sense.py:1066` | whether a batch scan re-enters a file |
| `observe/describe.py:1577`, `:178` | handler self-skip, and which rows an incremental re-run reuses |
| `think/data_state.py:54` | the shared modality state, consumed by `think/cluster.py:503` and `apps/transcripts/routes.py:747` |
| `think/retention.py:133` | 🔴 **irreversible raw-media deletion** |
| `apps/observer/processing_proof.py:61` | 🔴 **that a device may drop its local copy** |
| `core/crates/solstone-core-ingest-resolve/src/terminal_proof.rs:54` | the same, in Rust |
| `think/backfill_processing_records.py:158` | its own skip guard |

🔴 **The version string exists twice** — `SCHEMA` (`processing_record.py:23`) and `PROCESSING_SCHEMA` (`terminal_proof.rs:11`), hand-maintained with nothing binding them. **Removing that is this strand's first job.**

⚠ The two terminal-proof readers require more than the schema does: `processing_proof.py:64-76` and `terminal_proof.rs:57-63` both check `schema` match, `state ∈ {analyzed, empty}`, `handler` matching the extension, **and `input_size == recorded_size`**. That conjunction is the contract for releasing owner data, and it is written down in neither place.

🔴 **But the conjunction is NOT what gates the other irreversible decision, and reading it as though it were is the trap.** Measured 2026-08-05 by tracing `think/retention.py:133` into `think/data_state.py:121-158`: the retention path reads **`record.get("state")`, plus `is_failure_exhausted(record)` on the failed branch, and nothing else.** ⛔ No `schema` check, no `handler` check, no `input_size` check. **A record consisting of `{"state": "empty"}` and nothing else is enough to purge an owner's raw media.** The stricter predicate protects the device's copy; the weaker one deletes the journal's.

🔴 **And the two terminal-proof implementations already diverge, in the direction that releases owner data.** Condition 3 is parsed differently on each side — Python uses `Path(name).suffix` (`processing_proof.py:26`), Rust uses `name.rsplit_once('.')` (`terminal_proof.rs:71-78`). The extension *sets* match exactly; the *parsers* do not. For a name whose only dot is leading — `.mp4` — Python's suffix is `""` and it refuses at the first branch, while Rust yields `"mp4"` and proceeds to grant proof. ⚠ **Reachable:** `ContentName::new` accepts `.mp4` (`solstone-core-segment/src/content_name.rs:41-55` rejects only empty, `/`, `\`, `.`, `..` and reserved names) and it is built straight from the client-submitted filename (`solstone-core-ingest/src/router.rs:324`, `:427`). So a device can be told it may delete its only local copy on a name the reference would never have proven. ⛔ **A rebuild narrows to the reference here; widening proof is the one direction that loses owner data.** 📌 The sub-shape worth carrying: Python holds the extension sets **dotted**, Rust holds them **undotted** in a hardcoded `match` — two representations of one set, which is why the parsers could drift without anyone noticing the sets had not.

⚠ **Re-entry is describe-only in practice.** `should_reenter_analysis_output` (`processing_record.py:118-152`) returns `True` only for `handler == "describe"`, and transcribe's own decode-failure writer then blocks re-entry at three separate guards — so `FAILED_ATTEMPT_BOUND` (3) never applies to audio. ⛔ Whether that asymmetry is intended is a contract question this strand owns, not a bug to fix silently: `tests/test_data_state.py:145` encodes it as deliberate, while the *other* transcribe failure paths write no record at all and re-pay decode + VAD + STT forever.

**Carry forward:** the closed sets are the contract — `state ∈ {analyzed, empty, failed}`, `reason_code ∈ {ok, no_decodable_frames, no_decodable_audio, corrupt_input, analysis_failed}`, `handler ∈ {describe, transcribe}` — and `corrupt_input` is terminal immediately while everything else exhausts at `attempts >= 3`.

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

📌 **`P-segment-sense` produces on three tracts only** — `observe` (`detected`, `observed`, `status`, `described`, `transcribed`, `memory_throttle_started`, `memory_throttle_completed`), `notification` (`show`), and `supervisor` (`request`, from `observe/transfer.py:437`). ⛔ **`observe.observing` is NOT this plate's** — it is the ingest side's trigger *into* it (`apps/observer/routes.py:1289`, `think/importers/cli.py:1243`).

### `S:segment-sense:journal-segment-events`
**Connects** `P-segment-sense` → `P-journal` · **Owner** `P-journal` · **Tier** fixture

The **durable** half of the callosum contract, split from the wire half above. Bus envelopes are appended verbatim into `{day}/[{stream}/]{segment}/events.jsonl`.

⚠ **The bus writer bypasses `journal_io`** — bare `open(…, "a")`, failures swallowed at `logging.debug` — and 🔴 **it is not in this plate at all: it is `think/supervisor.py:6034-6068`**, wired as a callosum client callback (`:6763-6764` → `:6132-6141`). A rebuild looking for it inside sense will not find it. It filters to tracts `observe`, `think`, `activity` (`:6040`), requires `day` and `segment` (`:6046`), and **silently drops a well-formed event whose segment directory does not exist yet** (`:6058-6059`) with no log line at any level. The `journal_io` helper it bypasses (`think/journal_io/append.py:12-37`) is the one that fsyncs per record and fsyncs the parent on create, and seven other modules use it — this is an outlier, not an era.

🔴 **CORRECTED 2026-08-05 — three things this entry said, measured against the tree:**

1. **There are TWO writers with OPPOSITE discipline, appending to the same file.** The second is Rust: `core/crates/solstone-core-segment/src/sidecars.rs:9-13` goes **through** `solstone_core_journal_io::append_jsonl` and **propagates** — its caller turns a failure into a `500` (`solstone-core-ingest/src/router.rs:511-518`). ⛔ The one write discipline this entry asks for is not merely absent; the tree already contradicts itself on it.
2. **The file is NOT one shape, in two separate ways.** ⚠ *Within* the bus family the key set already varies per emission — measured on a real journal, one segment's 33 rows are all `think.status` and carry **two different key sets**, because the envelope is `{**defaults, tract, event, **fields}` and `fields` is whatever the emit site passed. ⛔ So the durable model is open and preserving, never a fixed struct. *Across* families, the Rust ingest path appends `DeviceIngestEvent` (`solstone-core-ingest/src/model.rs:155-167`) — `record_type`/`record_version`/`outcome`/`did`/`source`/`files`, and **no `tract`, `event` or `ts`**. The Rust reader discriminates on `record_type` (`solstone-core-ingest/src/events.rs:21-23`); the Python readers discriminate on `tract` (`think/segment.py:194-196`), so each family is invisible to the other's readers. ⚠ **Measured 2026-08-05: no journal contains both today** — the `DeviceIngestEvent` writer has no live caller, so the cross-family case arms when the native ingest path goes live, and it is a landmine rather than a live defect. ⛔ A rewrite that models this file as a list of bus envelopes still silently drops every ingest attribution record the moment it does.
3. **It is NOT append-only.** `think/segment.py:380-409` rewrites the whole file on `journal segment move` — restamping `day` and `segment` on every parseable line, via `events.tmp` + `os.rename` (`:406-408`). ⚠ It has no `record_type` awareness and happens to be right about `DeviceIngestEvent` only because that row carries the same two keys. ⚠ And `events.tmp` is neither a reserved segment filename nor recognised as journal-derived, so a crash mid-move leaves a file `apps/observer/prune.py:683-698` reports as unknown.

⚠ **Readers are 4, not 2**, and `apps/observer/utils.py:1242-1261` is not one of them — it is a *filename predicate* that never opens the file. The real readers: `think/segment.py:176-198` (tolerant — skips undecodable lines), `think/segment.py:499-503`, and **`solstone-core-ingest/src/events.rs:10-28`**, which is **strict on the same bytes** — any malformed line fails the whole read with `IngestEventLogMalformed`. 🔴 A torn line from the non-fsynced Python append is survivable to one reader and fatal to the other.

⚠ **No rotation, no size cap, no compaction.** Retention never names this file (`think/retention.py:588-604` unlinks raw media only) and `log_retention.py` scans no segment directory. It grows unbounded for the life of the segment.

### `S:segment-sense:thinking`
**Connects** `P-segment-sense` → `P-thinking` · **Owner** `P-thinking` · **Tier** schema

⚠ The talent event vocabulary is **NDJSON on stdout** — a real inter-process wire — defined as `total=False` TypedDicts (`think/providers/shared.py:31-142`) with **no schema, no fixture and no validator anywhere in the tree**. **The shape most likely to be lost silently.** 📌 For contrast, the sibling vocabulary on the same bus *is* runtime-validated: `convey/chat_stream.py:137-138` raises on an unknown kind and `:375-386` enforces per-kind required fields. This one has the strictest static types in the tree and the weakest enforcement.

🔴 **CORRECTED 2026-08-05 — the wire and the persisted run log are NOT the same shape.** The producer-side sidecar writer is **dead in production**: `JSONEventWriter` accepts a path (`think/talents.py:142-152`) and the only production construction is `JSONEventWriter(None)` (`:2107`). The durable run log — `{journal}/talents/{name}/{use_id}_active.jsonl`, renamed to `{use_id}.jsonl` on completion — is written by the **consumer**, `think/cortex.py:707-733`, from what it parsed. It does not round-trip: cortex adds `use_id`, `name` and `day` (`:1200-1211`) and **synthesizes `info` records no talent ever emitted** — any non-JSON line becomes `{"event": "info", …}` at `:1289-1297` rather than an error. ⛔ Reading the run log as a recording of the wire overstates it in both directions.

⚠ **The TypedDicts are `total=False` but five of the eight carry `Required[]` markers** — `StartEvent`, `FinishEvent`, `TalentUpdatedEvent`, `ThinkingEvent`, `TextDeltaEvent`. That is a real static contract, simply never checked at runtime. ⚠ And the `Event` union **under-covers the wire**: `warning` (`providers/cli.py:425`), `tool_budget_exhausted` (`providers/cli.py:565`, `providers/openhands.py:858`) and `info` are emitted with no TypedDict at all.

⚠ **Every event name on this wire becomes a `cortex.*` bus event by variable** — `cortex.py:1216-1224` pops `event` from the parsed line and relays it. So this strand's vocabulary is the hidden source of `S:segment-sense:system`'s largest grep-invisible drift class, and `cortex.unknown` is reachable from any talent line lacking an `event` key (`:1219`). ⚠ Terminal detection is by name — `finish`, or `error` whose `terminal` **defaults to `True` when absent** (`:1241-1244`), so a provider omitting an optional field kills the run.

### `S:thinking:journal-thinking`
**Connects** `P-thinking` → `P-journal` · **Owner** `P-journal` · **Tier** fixture

Talent output landing durably. Carries the failure semantics: retries, back-offs, days being complete, segments being complete.

**Carry forward:** the talent-use lifecycle where **the filename is the lock and the state** — exclusive `open(…, "x")` on `{use_id}_active.jsonl` is the claim, rename on completion, and on restart every leftover `_active` is terminalized with an error so an interrupted talent is never indeterminate.

🔴 **`_active.jsonl` is not only a talent convention — it is a deletion gate in two other subsystems.** Seven production readers, including `think/log_retention.py:368` (skips pruning them) and **`think/retention.py:199` (treats presence as "segment incomplete, do not purge raw media")**. A cleaner claim filename is a format change on the writer side and a **silent capability loss on the deletion side.**

### `S:thinking:local`
**Connects** `P-thinking` → `P-local` · **Owner** `P-thinking` · **Tier** schema + fixture

The local model lane. ⚠ Not a types boundary — it is loopback HTTP **plus a durable record**. See [`plates.md`](plates.md) § `P-local` for the two things not to carry.

### `S:journal:index`
**Connects** `P-journal` → `P-index` · **Owner** `P-index` · **Tier** fixture

🔴 **Not already Rust** — see [`plates.md`](plates.md) § `P-index`.

⚠ **Fan-in is nine writers across six plates**, not one — the list previously named eight: `think/backup/restore.py:31`, `convey/chat_stream.py:182`, `think/importers/cli.py:27`, `think/day_accumulator.py:12`, `apps/observer/prune.py:27`, `apps/observer/share_delete.py:19`, `think/entities/merge.py:1234`, `think/segment.py:19`, and **`apps/search/maint/003_migrate_index_stream.py:34`**.

⚠ **A tenth call site bypasses the accessor entirely** — `think/segment.py:211` opens its own bare `sqlite3.connect(db_path)` rather than `get_journal_index()`, so it never runs the schema-ensure path. ⛔ Count it when counting callers of this boundary; an accessor-name grep misses it.

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
**Owner** ⚠ unassigned · **Tier** schema

Nine production readers across search, tools, voice, talents and entity context, plus three more on the edge half (`apps/home/connections.py`, `think/curation.py`, `apps/entities/routes.py`). The `search_journal` / `search_counts` interface — which `P-thinking` calls **directly, in-process, on every talent that searches** — belongs to no strand yet.

⚠ **The in-process talent call is worse than "a search":** `think/tools/search.py` calls **both** `search_journal` *and* `search_counts` on every invocation, on two separate connections, and `search_counts` returns every matching row to the caller to be counted in Python.

⛔ **`known_agents` is NOT on the talent path** — do not group it with the two above. `think/tools/call.py` returns early into `think/tools/search.py` when JSON output is requested, so `known_agents()` is reached only from the human CLI with an explicit `--agent`. Its cost is an owner-CLI cost, not a per-talent one. ⚠ It is still a full scan of the chunk table to list a set whose measured cardinality is 31.

### `S:*:P-entity` · `S:*:P-facet`
**Owner** `P-entity` · `P-facet` · **Tier** fixture

See [`plates.md`](plates.md) — this store fails by **bricking**, not degrading.

### `S:*:journal-config`
**Owner** `P-journal-config` · **Tier** fixture

See [`plates.md`](plates.md) § `P-journal-config` for the fail-closed posture that is the house style.

### `S:web:thinking` — chat
**Owner** `P-thinking` · **Tier** schema

The primary owner-facing use of the model. `convey/chat.py` (2,532 lines) + `chat_stream.py` (512) + `convey/sol_initiated/` (1,034). It **spawns talents**, so it is a producer into `P-thinking`, and it is in the audited native-client bundle — the capture clients depend on it.

⚠ **Push cannot be scoped until this exists** — push's trigger is the **chat tract** (`push/triggers.py:63-64`), so push is a callosum consumer downstream of the chat orchestrator, not a standalone journal→device path.

### `S:*:system` — the command channel
**Owner** `P-system` · **Tier** schema

`_handle_task_request` takes `message["cmd"]` off the unix socket and hands the argv to the task queue. **This is how the scheduler, importers, backup, and the sense/think pipeline all cause work to run** — six production producers. ⚠ Distinct from liveness/status.

### `S:journal:establish`
**Owner** `P-device-link` · **Tier** fixture

⚠ **Owner assigned 2026-08-05.** What establishment produces is the **identity root** — the promoted CA and the persisted instance identity — and `P-device-link` is the plate that owns identity and must serve every device that later pairs against that root. One-to-many end, so it owns the contract.

First-run journal establishment. **Creates the identity root** that `S:device-link:journal` depends on: the mark-lock route promotes the staged CA and persists the instance identity.

⚠ Eight `/init/*` routes are session-gate-exempt and admitted **before** the journal-is-active check. That is the same `localhost:5015` human-entry basis as everything else on `P-web`, before a session exists to gate — **not** a third access path.

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

🔴 **A removal request carries its own precondition, because consolidating the removers must not weaken the strongest one.** The offload pass archives to backup, **confirms the snapshot holds every byte at the recorded size**, appends its ledger, and only then removes — where retention's own path hashes the bytes and removes with no archive. When that removal becomes a request, the confirmed-snapshot precondition travels **with the request**. ⛔ A request type constructible without it has moved the guard out of the executor and into the caller.

⛔ **Two units, and the request names which.** Whole segments — one, or a set — leaving a `tombstone.json`; or the proven raw originals, leaving every derived output. ⛔ There is no third unit and no partial owner-directed delete. See [`plates.md`](plates.md) § `P-journal-retention`.

⚠ **`S:journal-retention:system` has two shapes that must not be conflated:** *when the policy sweep runs* — a schedule entry, `P-system`'s contract — and *a removal request arriving from another plate*, which is synchronous and needs no schedule. 🔴 **Today the first does not exist**: nothing schedules the raw-media pass, so two of the three configured retention modes never execute. See [`plates.md`](plates.md).

🔴 **`S:journal-retention:index` has no entry point to call.** The only path-keyed native index verb refuses a path that no longer exists, and the only prune is keyed by **stream name**, not by removed path — so a segment under one stream that held another source's originals keeps its rows after those originals are gone. The Python reference has the right shape (`think/indexer/journal.py:184-215`, `DELETE FROM chunks WHERE path = ? OR path LIKE ?`, segment-prefix-scoped) and has never been ported.

🔴 **Why the ordering is a safety property and not a preference.** Index discovery is a pure filesystem glob with no database input, and the scan deletes any row the glob no longer produces — so the index re-converges on the chronicle every run. **Remove, then notify:** a crash between them leaves stale rows the next scan deletes, because the file is gone; self-healing, and the owner is never told something is gone that is not. **Notify, then remove:** a crash leaves the index missing rows for a file *still on disk*, so in that window the owner has been told their data is out of search while it is still there — and the next scan **puts it back**. Only one order is safe from the owner's point of view.

Where retention is the provider it owns the contract — it is the one-to-many end, reached from **11 import sites across 7 production modules**. ⚠ Those 11 are not 11 decisions: one is the purge itself, two consult the deletion gate, and the rest read storage totals, the config, or a byte formatter. **Count the callers of the irreversible verb, not of the module.** See [`plates.md`](plates.md) for the irreversible-deletion carry-forward and the measured predicate.

---

## Tier 2

| Strand | Owner | Note |
|---|---|---|
| `S:system:system-health` | `P-system-health` | Liveness and current status of running things |
| `S:system-health:journal-health` | `P-journal` | 🔴 Per-day health JSONL grammar is entirely Python string literals |
| `S:web:journal` | `P-journal` | ⛔ 100% of `P-web` is `localhost:5015` or an authorized linked device |
| `S:cli-journal:journal` | `P-journal` | Same-device only; may modify the journal directly |
| `S:cli-sol:web` | `P-web` | Any device, API only, over a link. ⚠ May share `S:web:journal`'s contract |
| `S:segment-sense:speaker-id` | `P-speaker-id` | A refinement of sense |
| `S:speaker-id:journal-facet` | `P-journal` | ⚠ **Years of voiceprints must survive with no re-teach** — a shipped promise. **Carry forward:** the label-source resolver returns *ambiguous* and warns the owner rather than picking a source |
| `S:body-source:journal-body` | `P-journal` | Owner **body** data — ingress, not egress. ⚠ The shard format is defined only by its reader; 🔴 excluded from every backup with no rebuild path |
| `S:file-import:journal-segment` | `P-journal` | Generic import. ⚠ **A second write door into `chronicle/`** that consults no contract and writes with bare `write_bytes` — the create-exclusive guarantee is a property of **one** of the two doors |

## Tier 3 — additive, absent until approved in

`S:thinking:byo` — ⛔ egress, the owner's own key · `S:thinking:spp` — ⛔ egress, attested. ⚠ Fails closed by the *absence* of a fallback branch; make "no downgrade path" an explicitly tested invariant.

## Not yet placed

- **push** — ⚠ cannot be scoped until `S:web:thinking` exists; its trigger is the chat tract. Only reimplemented end-to-end encrypted.
- **encrypted backup** — blind by construction, but ⚠ **retention imports it**, so a core deletion path depends on it. 🔴 The 64-char recovery key **is** the repository password. **Carry forward:** the exclusion-list comment records a paid-for lesson about basename-at-any-depth matching that a rebuild would otherwise re-learn; and `brain.json` / `scheduler.json` / the supervisor-ready marker are excluded deliberately, so post-restore provider and scheduler state is an intentional blank.
- **support request** — ⛔ real egress; the `_SECRET_*` redaction is the last thing before an external service.
- **merge / transfer** — deferred. ⚠ `think/merge.py:30-33` imports three **private** functions out of `think/entities/merge.py`, so the frozen thing depends on the unstranded store.
