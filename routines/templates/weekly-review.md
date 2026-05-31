{
  "name": "weekly-review",
  "description": "Weekly reflection on themes, work completed, and planning signals from the last 7 days.",
  "default_cadence": "0 18 * * 5",
  "default_timezone": "UTC",
  "default_facets": []
}

You are writing a weekly review covering the last 7 days.

Gather evidence from the journal first, then synthesize a reflective but actionable markdown review.

## Gather

1. Use `sol call journal facets` to identify the facets in scope.
2. Use `sol call journal search "" --day-from $day_minus_7_YYYYMMDD --day-to $day_YYYYMMDD -n 25` to find notable entries and themes.
3. Use `sol call todos list` to review outstanding work and infer what likely got completed or deferred.
4. Use `sol call activities list --source anticipated --day YYYYMMDD` across the last 7 days to understand scheduled load and major time commitments.
5. Use `journal identity pulse` for the current state narrative.
6. Use `sol call journal news FACET --day YYYYMMDD` for any facet that needs a richer summary.

## Synthesize

- Identify the week's main themes and where attention actually went.
- Call out notable progress, stalled areas, and repeated friction.
- Note patterns in calendar density, follow-through, or context switching.
- Distinguish between signal and noise; do not produce a raw diary.
- End with 3-5 concrete priorities or questions for the coming week.

## Write

Structure the markdown with sections such as:

- `## Week in Review`
- `## What Moved`
- `## Friction and Gaps`
- `## Next Week`

Use bullets and short supporting sentences.
Anchor claims in the gathered evidence.
Keep the tone direct and reflective, not motivational.
