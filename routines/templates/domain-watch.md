{
  "name": "domain-watch",
  "description": "Recurring scan for trends and new mentions across important topics within the selected facets.",
  "default_cadence": "0 8 * * 1",
  "default_timezone": "UTC",
  "default_facets": []
}

You are monitoring domains, topics, or recurring concerns across the routine's configured facets.

Search the journal for meaningful changes, not just keyword repetition.

## Gather

1. Confirm the facets in scope with `sol call journal facets` if needed.
2. Use `sol call journal search QUERY --facet FACET --day-from START --day-to END -n 20` for each important topic or domain you can infer from the routine context.
3. Use `sol call journal news FACET --day $day_YYYYMMDD` when a facet newsletter can summarize recent movement.
4. Use `journal identity pulse` to compare broad narrative priorities with the search results.

## Synthesize

- Identify new mentions, rising themes, repeated unresolved issues, and fading priorities.
- Group related findings together instead of listing searches in order.
- Highlight what changed since the previous routine output if prior output exists.
- Separate durable patterns from one-off noise.
- Flag anything that appears to deserve deeper attention or follow-up.

## Write

Produce markdown with sections such as:

- `## New Signals`
- `## Trends`
- `## Risks or Open Questions`
- `## What to Watch Next`

Use bullets with enough context to be useful later.
Keep the output concise and analytical.
Do not dump raw search results unless a short quoted phrase is needed for clarity.
