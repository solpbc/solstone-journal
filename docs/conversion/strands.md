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

🔴 **A new sidecar name changes what the owner's location delete removes.** The delete classifies a segment as *location-only* (remove the whole directory) or *mixed* (remove one file) using `RESERVED_SEGMENT_FILENAMES` as its non-content set. **Every name added to that set silently reclassifies segments from mixed to location-only — silently widening deletion.** ⛔ Pin the derivation, never a literal list, and make an addition turn a test red rather than change behaviour quietly.

⚠ **`stream.json` is read far more widely than the plate's own notes suggest — 17 production call sites across 8 modules**, led by `think/segment.py` (6), `apps/settings/maint/*` (4) and `apps/observer/prune.py` (3), plus `think/cluster.py`, `think/streams.py`, `think/indexer/journal.py` and `observe/sense.py`. 📌 An earlier count of "six sites in `prune.py`" was counting error-message string literals, not readers. It matters because the marker carries `prev_day`/`prev_segment`/`seq`, and the reference writer **resets `seq` to 1 and stamps a fresh `created_at` whenever the stream record fails to parse** (`think/streams.py:214-232`, a swallowed `JSONDecodeError | OSError`) — forking the chain for all 17, silently, on a path that looks like a successful write.

🔴 **The content-identity reference refuses in three cases and silently degrades in the rest.** `_read_ingest_manifest` returns `{}` for *every* failure — unreadable, not JSON, non-dict root, unknown `schema_version`, non-dict `files`, or any entry that is not an object — and `content_identity_from_segment` reads that as falsy and **falls through to the legacy media-scan arm**, producing a valid identity computed from whatever is on disk. One of its own refusals is unreachable as a result. ⛔ **A rebuild must refuse instead**: exists-and-will-not-parse raises, missing returns defaults. The two refusals that genuinely protect owner media — on-disk bytes disagreeing with the manifest's `sha256`/`size`, and a manifest-named file absent without terminal processing proof — the reference does get right, and both must survive.

**Carry forward:** **unresolved error → hold raw**, via `exit 69` — the handler writes nothing, leaves the input, and the scanner records *neither* success nor failure. The deferral emitter deliberately swallows its own bus failure so a down bus cannot turn a deferral into data loss.

### `S:segment-sense:journal-segment`
**Connects** `P-segment-sense` → `P-journal` · **Owner** `P-journal` · **Tier** fixture

Processed sense output written back.

🔴 `_solstone_processing` has a version string and **no schema**, and is absent from both sibling schemas that enumerate every *other* header key. ⚠ **Read by five planes** — the fifth is `think/retention.py:133`, the highest-stakes one, because it decides irreversible deletion.

**Carry forward:** **terminal-empty written before the raw is unlinked** — die between them and the file survives with a terminal marker, never the reverse · atomic promote through a same-dir temp, header **last** · detections stored raw, filtered at read time.

### `S:segment-sense:system`
**Connects** `P-segment-sense` → `P-system` · **Owner** `P-system` · **Tier** schema

Callosum events emitted as processing happens.

There **is** a published machine-readable registry — `CALLOSUM_REGISTRY` (`convey/contract/assemble.py:43-99`), emitted as `x-callosum-registry`, 11 tracts and ~60 events, plus two published SSE operations. ⚠ **It is already drifted three ways:** events produced but undeclared · events emitted and prose-documented but absent from the registry · events declared with no literal producer. ⚠ The drift count is a floor — tracts passed through variables are invisible to a literal grep.

### `S:segment-sense:journal-segment-events`
**Connects** `P-segment-sense` → `P-journal` · **Owner** `P-journal` · **Tier** fixture

The **durable** half of the callosum contract, split from the wire half above. The whole bus envelope is appended verbatim into `{day}/[{stream}/]{segment}/events.jsonl`, with readers in `think/segment.py` and `apps/observer/utils.py`.

⚠ **That writer bypasses `journal_io`** — bare `open(…, "a")` — and swallows failures at debug level. ⛔ The segment-sidecar family wants one write discipline; do not let a new sidecar inherit this one.

### `S:segment-sense:thinking`
**Connects** `P-segment-sense` → `P-thinking` · **Owner** `P-thinking` · **Tier** schema

⚠ The talent event vocabulary is **NDJSON on stdout** — a real inter-process wire and the run-log format that gets persisted — defined as `total=False` TypedDicts with no schema, fixture, or validator. **The shape most likely to be lost silently.**

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

`P-journal-retention` connects through three strands, each a different contract:

| Strand | For | Owner | Tier |
|---|---|---|---|
| `S:journal-retention:journal-config` | the **posture / settings** it reads | `P-journal-config` | fixture |
| `S:journal-retention:system` | **when it runs** | `P-system` | schema |
| `S:journal-retention:journal` | **tending the files** — changes, and recording status | `P-journal` | fixture |

Where retention is the provider it owns the contract — it is the one-to-many end with ten production call sites. See [`plates.md`](plates.md) for the irreversible-deletion carry-forward.

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
