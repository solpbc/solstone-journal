# cables — whole use cases, end to end

**A cable connects a whole use case from one major system to another**, through several strands with plates between them. Named `C:plate:plate:plate…` — every plate in the path, in order.

Definitions and vocabulary: [`README.md`](README.md). Siblings: [`plates.md`](plates.md) · [`strands.md`](strands.md).

⚠ **A cable is a destination, not a work unit.** Its strands are. This file exists so the destinations stay visible while strands are built.

---

## `C:device-ingest:segment-media:segment-sense:thinking:journal-segment`

**A device's capture becoming a fully processed, thought-out result in the journal** — from device ingest, through sense media processing, through thinking, down to journal storage. The spine of the product.

Strands: `S:device-ingest:segment-media` → `S:segment-media:journal-segment` → `S:segment-sense:journal-segment` → `S:segment-sense:thinking` → `S:thinking:journal-thinking`

⚠ Every strand in this cable is Tier 1, so it completes when the core does.

## `C:journal:index:web:cli-sol`

**Journal content becoming answerable to an owner or a talent** — from journal storage, through the formatting layer, to the indexer, to the convey API, to the `sol` CLI.

Strands: `S:index:format` → `S:journal:index` → `S:web:journal` → `S:cli-sol:web`

---

## Candidate cables

Recorded so they are not silently dropped. ⛔ None is authorized as a destination yet.

- **link-to-trust** — `C:device-link:journal:web` — a device pairing and becoming visible to the owner as a trusted device.
- **body ingest** — `C:body-source:journal-body:web` — owner body data arriving and becoming readable.
- **honest state** — `C:system:system-health:journal-health:web` — the product reporting truthfully on itself, end to end.
