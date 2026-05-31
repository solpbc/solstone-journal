{
  "name": "monthly-patterns",
  "description": "Monthly analysis of recurring themes, focus shifts, and relationship activity over the past month.",
  "default_cadence": "0 9 1 * *",
  "default_timezone": "UTC",
  "default_facets": []
}

You are analyzing the last month of journal activity for recurring patterns and meaningful shifts.

Work at the month scale: look for durable changes in attention, habits, projects, and relationships.

## Gather

1. Use `sol call journal facets` to identify the facets in scope.
2. Use `sol call journal search "" --day-from START --day-to END -n 40` to survey the month across the configured facets.
3. Use `sol call journal news FACET --day YYYYMMDD` for representative weekly or recent snapshots when they help summarize a facet.
4. Use `sol call entities intelligence PERSON` for people who appear central to the month.
5. Use `sol call activities list --source anticipated --day YYYYMMDD` on representative days if scheduled load seems important.
6. Use `journal identity pulse` to compare month-long patterns against the current state narrative.

## Synthesize

- Identify recurring themes, repeated bottlenecks, and shifts in focus.
- Note whether energy moved toward or away from particular projects, people, or responsibilities.
- Highlight any relationship frequency changes that seem important.
- Compare early-month versus late-month signals when that reveals a trend.

## Write

Write markdown with sections such as:

- `## Month at a Glance`
- `## Recurring Patterns`
- `## Focus Shifts`
- `## Relationship Signals`
- `## Questions for Next Month`

Use concise bullets and short explanations.
Prefer pattern-level insight over day-by-day narration.
