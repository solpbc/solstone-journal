{
  "type": "cogitate",
  "title": "Weekly Reflection",
  "description": "Sunday-start weekly reflection synthesized from the journal",
  "schedule": "weekly",
  "priority": 90,
  "output": "md",
  "read_scope_span": 7,
  "max_turns": 100
}

$facets

You are generating the weekly reflection for $agent_name.

This is not a conversation. Gather what you need, synthesize the week, and return the reflection as markdown. The system saves your response automatically.

`$day_YYYYMMDD` is the canonical Sunday that starts the week under review. Cover that Sunday through the following Saturday.

Follow the provenance pattern from `solstone/talent/patterns/provenance.md`, including:
- a coverage preamble with source counts and gaps
- `sol://` attribution for consequential claims
- confidence-graded language that distinguishes observation from inference
- safe handling of tool errors and missing data

## Gather

Collect enough evidence to describe the week clearly. Prefer journal search and existing weekly/day outputs over broad transcript dumps.

Suggested sources:
1. `sol call journal facets`
2. For each active facet and relevant day in the week: facet newsletters and notable day-level outputs
3. `sol call journal search "" --day-from $day_YYYYMMDD --day-to <+6> -a pulse -n 12`
4. `sol call journal search "" --day-from $day_YYYYMMDD --day-to <+6> -a decisions -n 12`
5. `sol call journal search "" --day-from $day_YYYYMMDD --day-to <+6> -a followups -n 12`
6. `sol call activities list --source anticipated --from $day_YYYYMMDD --to <+6>`
7. `sol call todos list`
8. Entity or relationship lookups only when they materially improve the reflection

Before writing, audit your coverage:
- `newsletters`
- `activities`
- `decisions`
- `followups`
- `todos`
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
  todos: [count]
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
