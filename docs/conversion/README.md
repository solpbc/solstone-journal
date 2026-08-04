# The conversion dictionary — plates, strands, cables

**The canonical definitions of the journal's conversion boundaries.** If two pieces of work need to agree on what a boundary is called, what it holds, or which side owns its contract, it is defined here.

This directory is **definitional and present-tense**. It says what the boundaries *are*. It deliberately carries no schedule, no status, no ownership of work, and no record of who decided what — those live outside this repo, and anything of that kind appearing here is a defect.

**Companion:** [`../PORTING.md`](../PORTING.md) is the porting *doctrine* — how to port, layering, error mapping, data boundaries, JSON and hashing, version lockstep. This directory is the *map*. Read `PORTING.md` for how, read here for what and where.

## The files

| File | Holds |
|---|---|
| [`plates.md`](plates.md) | every plate, what it holds, its contract tier |
| [`strands.md`](strands.md) | every strand, its two plates, which end owns the contract, and what must be carried forward |
| [`cables.md`](cables.md) | whole use cases through several strands |

## Vocabulary

| Term | Means |
|---|---|
| **plate** | A boundary of the journal where strands connect from other plates. ⛔ A plate has no "sides" — strands can be bi-directional, and in/out gets confusing |
| **strand** | The minimum viable path connecting two plates, in Rust, robust. May be bi-directional. **The contract lives at one *end* of the strand, never both** |
| **cable** | A whole use case, from one major system to another, through several strands with plates between them |

⛔ **"Surface" is not one of these.** It is reserved for **owner-visible** things. ⛔ Not "thread" either, so it never collides with the Rust threading model.

## Naming

| | |
|---|---|
| Plate | `P-name` — ⛔ never `P1`/`P2`; a number reads as a priority |
| Sub-plate | dashes add granularity within a plate: `P-journal-segment` |
| Strand | `S:plate:plate` — 🔴 **the second plate always owns the contract** |
| Cable | `C:plate:plate:plate…` — every plate in the use case, in order |

Worked: `S:device-ingest:segment-media` — `segment-media` owns the contract. `C:device-ingest:segment-media:journal-segment`.

## One contract per strand, at one end

**The owner is the one-to-many end.** It must serve all comers and cannot negotiate per-peer. **Tiebreak when the strand is one-to-one: the durable end owns it.** By convention the owner is written second in the strand name.

📌 *Why this is a rule and not a preference:* nearly every contract drift in this tree is two places owning one thing with nothing binding them — a relay string matched verbatim on both sides, a nonce alphabet hand-copied between languages, a prompt preamble held as a constant in one place and as a sha256 of itself in another, a frontend switching on phases its backend enum does not have. One owner per contract makes that class **unrepresentable** rather than merely detectable.

## Contract tiers — by what kind of boundary it is

| Tier | Is | Example |
|---|---|---|
| **types** | types in a binary | — |
| **schema** | an API or interface format | the OpenAPI contract, the callosum envelope |
| **fixture** | a durable storage format | the `x-journal-contract` files, the path-resolution vectors, the identity vectors |

⛔ Not graded by enforcement strength — graded by the **kind** of boundary. A vector is a type of fixture.

## Writing and reading

🔴 **Write new, read old.** A new implementation does not have to write what Python wrote, as long as there is a contract for what it *does* write. When it **reads**, it stays compatible with the older format. Bit-identical writes are **not** required. ⛔ The one thing that is never acceptable is leaving older journal data unseen.

🔴 **Identity is written beside the data, never re-derived from a name or a position.** Where a derived value doubles as a *lookup key*, writing it differently forks the namespace instead of just changing a format. **The answer is not to teach every reader two spellings — it is to stop deriving:** persist the identity next to the data, and let the derived string be a **label** that nothing resolves by.

`device.json` in a segment directory is the worked example: the stream name became human-friendly precisely *because* it stopped being load-bearing.

⚠ **Read-compat still applies to old data** — a reader handles records written before the identity was persisted. But **a writer never re-derives.**

Known instances and what is left of each:

| Instance | State | Residue |
|---|---|---|
| `entity_slug()` | `id` is written into `entity.json` on create (`think/entities/journal.py:59`, `:201`); `loading.py:86` reads `data.get("id") **or** entity_slug(name)` — the derivation is a **fallback** | the fallback branch, and the `entities/{entity_id}/` directory name |
| `ambiguity_id` | required on the row and hard-rejected when absent (`ambiguities.py:157-159` raises `missing ambiguity_id`) | essentially none |
| `sentence_id` | 🔴 **the real instance** — a 1-based ordinal recomputed at read time and stored nowhere | all of it |

**The work: make the written identity mandatory, delete the derivation fallback, persist `sentence_id`.**

## 🔴 Reserved words — two live collisions

Two words mean an internal thing and an owner-facing thing at once, and **both collisions are already in shipped code.**

| Word | ✅ Means ONLY | ⛔ Never means | Use instead |
|---|---|---|---|
| **health** | the **journal system's** health — is it running, is everything processed | the owner's physical health | **body** |
| **activities** | the **internal** facet activity model (`facets/{facet}/activities/{day}.jsonl`) | the owner's physical movement | **body motion · fitness · kinetics** |

**All owner physiological data is `body`** — *body data*, *body records*, *body sources*.

⚠ `apps/body/routes.py:1985` defines `_activity_analysis` over *wearable* activity while `think/activities.py` owns the internal model — one word, two meanings, live today.

⛔ **Consequence for plate names:** `P-journal-health` is **system** health written durably. Owner body records are **`P-journal-body`**, a different plate. Never fold them.

## Changing these files

**The plate set and the strand definitions are fixed.** Within a boundary you own, correct a fact, refine a contract note, or record what must be carried forward — that is expected and welcome.

⛔ **Do not, on your own initiative:** add, split, rename or re-scope a **plate**; change a **strand's** definition, tier, or which end owns its contract; define or re-scope a **cable**. Those are decided outside this repo. If the code contradicts a definition here, that is worth raising — say so plainly rather than quietly widening the definition to fit.

⚠ **Every count and inventory here is a floor, not a census.** Two string-keyed dispatch registries resolve modules by name at call time with **zero static import edges** — `FORMATTERS` (`think/formatters.py:139-265`, reaches 12 modules) and `EDGE_SOURCES` (`think/edge_sources.py:39-73`, reaches 6), both via `import_module` + `getattr`. No import graph over this tree is complete. **A boundary is not clean until you count its callers.**
