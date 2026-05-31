{
  "name": "energy-audit",
  "description": "Weekly reflection on where energy went — productive depth, meetings, context-switching, and drift.",
  "default_cadence": "0 17 * * 5",
  "default_timezone": "UTC",
  "default_facets": []
}

You are preparing a weekly energy audit — a reflection on where the owner's time and attention actually went, versus where they intended it to go.

## Gather

1. Use `sol call activities list --source anticipated --day YYYYMMDD` for each of the past 7 days to map scheduled load.
2. Use `sol call journal search "" --day-from START --day-to END -n 30` to survey activity patterns.
3. Use `sol call todos list` to compare intended work against actual activity.
4. Use `journal identity pulse` for the current state narrative.
5. Use `sol call journal news FACET --day YYYYMMDD` for representative days across active facets.

## Synthesize

- Map the week into blocks: deep work, meetings, reactive work (email/messaging), context-switching, and drift (time that went somewhere unintentional).
- Compare meeting-heavy days against productive-output days. Is there a pattern?
- Identify the longest unbroken focus blocks and what enabled them.
- Note context-switching patterns — rapid jumps between facets or activities that fragment attention.
- Look for drift: time that didn't clearly serve any active priority or intention.

## Write

Structure as a reflection:

- `## Where Energy Went` — The week in broad strokes: how much deep work, how many meetings, how much reactive time
- `## Best Blocks` — The most productive stretches and what conditions enabled them
- `## Fragmentation` — Where context-switching or interruptions cost the most
- `## Drift` — Time that didn't serve stated priorities (not a judgment — just visibility)
- `## One Adjustment` — A single concrete suggestion for next week based on the patterns

Keep the tone observational, not motivational.
Use calendar and activity evidence, not assumptions.
The owner knows what they meant to do — this shows them what they actually did.
