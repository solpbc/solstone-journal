{
  "name": "relationship-pulse",
  "description": "Review relationship health and identify people who need attention or follow-through.",
  "default_cadence": "0 9 * * 1",
  "default_timezone": "UTC",
  "default_facets": []
}

You are reviewing relationship health across the routine's configured facets.

Focus on people who matter operationally or personally, especially where contact, follow-through, or momentum has changed.

## Gather

1. Use `sol call journal facets` if you need to confirm the active facet set.
2. Use `sol call journal search "" --facet FACET -n 20` to identify frequently mentioned people or recent interactions in each relevant facet.
3. For each meaningful person, call `sol call entities intelligence PERSON`.
4. Use `sol call journal news FACET --day $day_YYYYMMDD` if a facet summary helps explain current context.
5. Use `journal identity pulse` for broad priorities that may affect relationship maintenance.

## Synthesize

- Identify strong, active relationships versus neglected or at-risk ones.
- Note recent interactions, open loops, and people who likely need a reply, check-in, or prep.
- Prioritize by importance and recency, not by raw mention count.
- Distinguish between work relationships, collaborators, and personal contacts where relevant.
- For the 2-3 most active relationships this week, note not just frequency but
  quality signals: Are conversations getting deeper or more transactional? Is
  initiative balanced or one-sided? Are there topics being avoided?
- End with one reflective observation: a relationship trend the owner might not
  see from the inside.

## Write

Write markdown with sections such as:

- `## Active Relationships`
- `## Needs Attention`
- `## Open Loops`
- `## Suggested Next Moves`
- `## Reflection` — One honest observation about a relationship pattern the data reveals

Keep each person entry short and specific.
Use entity intelligence to ground your judgments.
Avoid generic advice; tie every recommendation to journal evidence.
