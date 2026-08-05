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

## `P-segment-sense`

Media processing. ⚠ Emits **two** strands with different contracts — processed data back to storage, and callosum events out.

## `P-index`

The SQLite index. **Ephemeral by design and always rebuildable — that property is required, not incidental.** ⚠ The index schema needs architecture work.

🔴 **Half of it IS already Rust, and the half that is has the larger share of the code.** ⛔ Do not read `think/indexer/native.py:6-11` as "this plate is Python" — it is accurate about what went native and silent about how much. `core/crates/solstone-core-indexer` (11,825 lines) + `solstone-core-indexer-store` (4,411) = **16,236 lines of Rust owning the entire CLI write path** — `--reset`, `--rescan`, `--rescan-full`, `--rescan-file`, `--rebuild-edges`. That is 5.5× the Python it fronts (`indexer/journal.py` 1,693 + `edges.py` 1,263). **A full rebuild is already native.** What remains Python is **the whole read/query path** plus the in-process writers.

🔴 **The schema DDL exists in two hand-maintained copies** — `think/indexer/journal.py:SCHEMA` and `core/crates/solstone-core-indexer-store/src/db.rs` (`CREATE_FILES` · `CREATE_CHUNKS` · `CREATE_EDGE_FILES` · `CREATE_EDGES` + the three edge indices). `db.rs:27` names the Python side as source of truth **for the edges half only**; the `chunks` DDL carries no such note. This is the two-places-one-contract class inside the plate whose schema is the thing being redesigned.

⚠ **Rust's `ensure_schema` has no equivalent of Python's `time_bucket` rebuild check** and its own comment says it relies on `--reset` instead. A pre-`time_bucket` index reached by the native path first gets `CREATE VIRTUAL TABLE IF NOT EXISTS` as a no-op, then an 8-column insert against a 7-column table.

**Shape of the live schema:** one FTS5 virtual table (`content` + **seven `UNINDEXED` columns** — `path`, `day`, `facet`, `agent`, `stream`, `idx`, `time_bucket`), a `files(path, mtime)` staleness watermark, and the derived `edges` / `edge_files` pair. 🔴 **Every metadata filter is therefore a post-filter over the whole match set, and a filter with no search term is a full table scan** — `_build_where_clause` emits `1=1` for an empty query. The `edges` half, which does have real indices, is the existing proof the same file can serve indexed lookups.

**Carry forward — measured on a large populated journal (2.83M chunk rows, 1.64 GB, 439 days):**

- 🔴 **FTS5 `optimize` is never run anywhere in either implementation, and the scheduler does not run it.** On a corpus with ~98k write transactions this left **34% of the file** as unmerged-segment fragmentation: the inverted index measured 695.7 MB where a single-pass rebuild of the identical rows measured 208.1 MB, and `optimize` + `VACUUM` recovered it in 7.6 s. Whatever the new schema is, **index maintenance has to be part of it** — this is not a schema flaw, it is a missing operation.
- 🔴 **The segment aggregate double-indexes its own children.** `_index_segment_chunks` re-concatenates a segment's `talents/*.md` under `agent='segment'` while those files are also indexed individually. Measured: **48.2% of rows and 41.1% of indexed text**, with **100% of aggregate paths also having their children indexed separately.** ⚠ It is not pointless — it buys phrase/`NEAR` matching *across* talents within one segment, which per-file chunks cannot serve. ⛔ But the read path then spends two `SELECT DISTINCT path` scans per query undoing it, and materializes the result as **one bind parameter per aggregate path** against SQLite's 32,766-variable ceiling — measured at 24,127 on that corpus, growing one per segment recorded. A redesign must decide whether that recall capability survives, and if it does, carry it as **written segment identity on the chunk row**, never as a query-time `IN` list.
- ⚠ **`day` is the dominant query axis** (the owner surface is day-grouped and day-paginated, three of eight filter parameters are date bounds) and is stored as unindexed `TEXT` compared with `>=`/`<=`.
- ⚠ **Aggregation is part of every read**, not a separate feature — results are always paired with counts by facet/agent/day/stream, and today that is done by pulling every matching row into the application.

## `P-format`

Consistent formatting of **structured journal data** for its consumers — the indexer and the convey apps.

🔴 **No import graph shows this plate's fan-out.** `FORMATTERS` (`think/formatters.py:139-265`) reaches 12 modules by **string key** via `import_module` + `getattr` (`:283-286`), with zero static import edges. It is the de facto read-side inventory of every on-disk shape, and it lives only in Python.

## `P-thinking`

🔴 **A grouping plate.** Holds **two contracts: `generate` and `cogitate`**. Everything connects to it. `P-local`, `P-BYO` and `P-SPP` sit behind it.

⚠ The runtime preamble every talent is written against exists as a **sha256 only** in the cross-language fixture — drift is detectable, the text is not reproducible.

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
