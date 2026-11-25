{
  "type": "generate",

  "title": "experiences detect",
  "description": "arbitrates cross-facet experience candidates and applies quality gates for a single day",
  "color": "#4338ca",
  "schedule": "daily",
  "priority": 65,
  "output": "json",
  "schema": "experiences_detect.schema.json",
  "hook": {"pre": "experiences_detect", "post": "experiences_detect"},
  "load": {"transcripts": false, "percepts": false, "talents": false},
  "temperature": 0.1,
  "max_output_tokens": 1536
}

# Cross-Facet Experience Arbitration

You receive all candidate experiences proposed across the day's enabled facets, with exact cross-facet shared-segment overlap metadata (`SHARED_SEGS_WITH: ...`).

Issue one verdict (`keep`, `drop_duplicate_cross_facet`, `drop_passive_spectacle`, or `drop_routine_or_trivial`) for every candidate `cand_id`:

1. **Cross-facet deduplication (`drop_duplicate_cross_facet`):**
   - Use `drop_duplicate_cross_facet` only when `SHARED_SEGS_WITH:` is present (two candidates in different facets share underlying segments) and both describe the same real-world event.
   - Always mark one candidate (the single best primary facet) `keep` and set `duplicate_of` on secondary copies to the kept `cand_id`. Never drop both sides.
   - Keep both facets only when the second facet carries genuinely distinct substantive topics on its own non-overlapping segments.
   - Never use `drop_duplicate_cross_facet` when `SHARED_SEGS_WITH:` is absent.
2. **Passive media or spectacle gate (`drop_passive_spectacle`):**
   - Drop when the owner was passively viewing or listening to TV, drama, sports broadcasts, podcasts, overheard street performances, or silently attending a show where fictional or stage characters spoke without active participation by the owner.
   - Do not drop active human conversations, meals, hangouts, or rehearsals and performances the owner actively participated in.
3. **Solo milestone bar and trivial exhaust gate (`drop_routine_or_trivial`):**
   - Drop ordinary single-focus solo coding, terminal, optimization, copying, or folder-review blocks without a clear deliverable, and trivial 1-3 minute micro-tasks (passing remarks, quick login/banking/medical checks, single ledger entries, routine single-turn messages).
   - Keep genuine standalone or multi-activity conversations, meals, meetings or calls, outings or errands, active rehearsals or performances, and clear solo creative or engineering deliverable milestones.

## Candidates

$candidates_packet

## Output

Return only a JSON object matching the schema with one verdict entry per input `cand_id`. Do not wrap in a markdown fence.
