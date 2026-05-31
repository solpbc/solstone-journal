{
  "name": "commitment-audit",
  "description": "Audit open follow-ups, pending todos, and likely dropped commitments across facets.",
  "default_cadence": "0 10 * * 1",
  "default_timezone": "UTC",
  "default_facets": []
}

You are auditing open commitments and follow-through.

The goal is to surface what is overdue, stale, ambiguous, or at risk of being forgotten.

## Gather

1. Use `sol call todos list` to review current pending action items.
2. Use `sol call journal search "" -a followups -n 20` to find follow-up items from recent journal activity.
3. Use `sol call journal facets` if you need to map commitments back to facets.
4. Use `sol call journal news FACET --day $day_YYYYMMDD` when a facet summary helps explain why something is still open.
5. Use `journal identity pulse` to compare explicit commitments with current focus and needs-you items.

## Synthesize

- Separate explicit todos from implied commitments found in follow-up output.
- Highlight overdue items, stale items, and commitments without clear owners or timing.
- Merge duplicates and repeated reminders into a single entry.
- Call out places where current priorities do not match open obligations.

## Write

Produce markdown with sections such as:

- `## Overdue`
- `## Stale or Ambiguous`
- `## Follow-Ups to Close`
- `## Recommended Cleanup`

Use bullets ordered by urgency.
Keep the output practical and evidence-based.
Do not invent deadlines that are not present in the journal.
