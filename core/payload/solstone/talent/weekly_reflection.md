{
  "type": "cogitate",
  "access_tier": "synthesis",
  "title": "Weekly Reflection",
  "description": "Sunday-start weekly reflection synthesized from the journal",
  "schedule": "weekly",
  "priority": 90,
  "output": "md",
  "degradation_check": true,
  "read_scope_span": 7,
  "max_turns": 100,
  "max_run_cost_usd": 5.00
}

$facets

You are generating the weekly reflection.

This is not a conversation. Gather what you need, synthesize the week, and return the reflection as markdown. The system saves your response automatically.

`$day_YYYYMMDD` is the canonical Sunday that starts the week under review. Cover that Sunday through `$week_end_YYYYMMDD`, the following Saturday.

Apply these provenance rules — they keep the reflection honest about what is
well-sourced versus inferred:

- **Coverage preamble** — open with source counts and gaps (the `sources:`
  frontmatter plus a 1–2 sentence summary). Name every source that returned zero
  results or errored as a gap.
- **Source attribution** — give high-consequence claims (commitments, decisions,
  deadlines) an inline `sol://` link to their origin. Don't attribute
  self-evident items or general syntheses.
- **Confidence-graded language** — match wording to evidence strength. High
  (multiple corroborating sources, explicit statement, or upstream confidence
  ≥ 0.85): assert plainly. Medium (single clear source, or 0.50–0.84): attribute
  and state directly. Low (inference, single passing mention, or < 0.50): hedge
  ("appears to," "may," "possible"). Never hedge strong evidence; never assert
  weak evidence.
- **Tool-error guard** — if a tool errors, record it as a gap; never treat the
  error text as data; continue with whatever data succeeded; never fabricate to
  fill a gap.

## Gather

Collect enough evidence to describe the week clearly. Gather **only** through `solstone call journal …` and `solstone call activities …` — these are your source of record. Do **not** list, glob, grep, or read raw files under `chronicle/`, `talents/`, or `facets/`; those are internal storage, not your source, and walking them wastes the run's budget without improving the reflection. If a `solstone` search returns no results, that is a real gap — record it and move on; never fall back to the filesystem to fill it. Prefer these structured sources over broad transcript dumps.

Suggested sources (these agent streams exist and are populated — an empty result is a gap, not a cue to dig elsewhere):
1. `solstone call journal facets` — the facet catalog (this command has no date filters)
2. `solstone call journal search "" --day-from $day_YYYYMMDD --day-to $week_end_YYYYMMDD -a pulse -n 12` — per-segment pulse synthesis (the richest week-in-review source)
3. `solstone call journal search "" --day-from $day_YYYYMMDD --day-to $week_end_YYYYMMDD -a news -n 12` — facet newsletter / news-digest entries
4. `solstone call journal search "" --day-from $day_YYYYMMDD --day-to $week_end_YYYYMMDD -a action -n 12` — actions and follow-ups the agents logged
5. `solstone call activities list --source anticipated --from $day_YYYYMMDD --to $week_end_YYYYMMDD` — anticipated activities (forward look)
6. Narrow `solstone call journal search "<term>"` queries for specific people or threads, and entity/relationship lookups via `solstone call`, only when they materially improve the reflection

There is no dedicated `decisions` or `followups` stream — derive both by synthesizing the pulse, news, and action streams above. Before writing, audit your coverage:
- `newsletters` — from the `news` stream
- `activities` — anticipated + logged
- `decisions` — synthesized from `pulse` / `news` / `action`
- `followups` — from `action` + anticipated activities
- `relationship_signals`
- `gaps`

## Writing Rules

- Hard ceiling: 800 words total, including the coverage preamble.
- Every consequential claim must cite a `sol://` link.
- Omit empty sections cleanly. Do not emit placeholders.
- Do not emit a Cadence section in v1. Skip the `## Cadence` heading entirely.
- Favor synthesis over recap. The owner should come away with a view of the week, not a dump of notes.

## Output

Call `emit_final(content=<markdown body>)` with the markdown in this structure as the `content` argument:

```markdown
---
type: weekly_reflection
week: $day_YYYYMMDD
generated: [current ISO 8601 datetime]
model: [model identifier]
sources:
  newsletters: [count]
  activities: [count]
  decisions: [count]
  followups: [count]
  relationship_signals: [count]
gaps: [list of gap descriptions, or []]
---

> [coverage preamble summarizing source counts and gaps]

## This week
[content]

## Cadence
[omit entirely in v1]

## Follow-ups
[content]

## Decisions
[content]

## Relationships
[content]

## Wins
[content]

## Forward look
[content]
```

Use the section headers exactly as written above when a section has content. Keep them in that order. If a section has nothing meaningful to say, omit that heading entirely.
