{
  "name": "meeting-prep",
  "description": "Prepare a concise briefing before an upcoming anticipated activity using participant and topic context.",
  "default_cadence": {"type": "activity-anticipation", "offset_minutes": -30},
  "default_timezone": "UTC",
  "default_facets": []
}

You are preparing for an upcoming meeting.

The routine prompt already includes an `Upcoming Activity` section with the title, start time, participants, and description. Use that activity context as the anchor for all research and synthesis.

## Gather

1. Read the upcoming event details in the prompt carefully.
2. If you need broader context, call `sol call activities list --source anticipated --day $day_YYYYMMDD` to see the surrounding schedule.
3. For each listed participant, call `sol call entities intelligence PERSON --brief`.
4. Use `sol call journal search QUERY -n 10` to look for recent mentions of the meeting topic, project, or participants.
5. If a configured facet seems especially relevant, use `sol call journal news FACET --day $day_YYYYMMDD`.
6. Use `sol call todos list` only if pending action items are directly relevant to the meeting.
7. Use `journal identity pulse` if it helps connect the meeting to current priorities or tensions.

## Synthesize

- Summarize who is involved and what matters about each participant.
- Identify recent context that is likely to come up.
- Note open loops, decisions pending, and useful reminders.
- Surface risks, unresolved questions, and preparation gaps.
- Keep the brief short enough to read right before the meeting.

## Write

Write markdown with sections such as:

- `## Meeting`
- `## Participant Context`
- `## Likely Topics`
- `## Open Questions`
- `## Prep Notes`

Use bullets and short sentences.
Do not repeat the full raw event block unless needed for clarity.
Focus on what will help right before the meeting starts.
