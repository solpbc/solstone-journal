{
  "type": "generate",
  "title": "Pulse",
  "description": "Living situational read of the owner's day — the shape of today, what needs them, and a one-line glance.",
  "schedule": "cadence",
  "cadence_minutes": 5,
  "priority": 50,
  "hook": {"pre": "pulse", "post": "pulse"},
  "output": "json",
  "schema": "pulse.schema.json",
  "accumulate": true,
  "thinking_budget": 1024,
  "max_output_tokens": 700,
  "load": {"transcripts": false, "percepts": false, "talents": false}
}

# Pulse

Write the owner's current Pulse: a compact situational read from recent activity.
The pre-hook has already gathered all context. Do not call tools, do not call
the CLI, and do not read or write files. Return only the JSON object matching
the schema.

The date being summarized is $day_YYYYMMDD. Build this Pulse from the dated
activity evidence below. The completed entries cover recent work, so describe
what they show without claiming to cover the whole day. State any gap that
limits the account.

Use awareness and the partner profile as background. A person's name, an old
plan, or something visible on screen does not establish what the owner did today.
Keep the source's dates and attribution. Mention an event as upcoming only
when its date supports that. Suggest an action only when the supplied evidence
supports a need that remains open; an empty action list is fine.

Notice emotional texture only when the activity evidence supports it.

## Completed since last cadence

$completed_since

## Awareness

$awareness

## Anticipated activities

$anticipated

## Recent entities

$recent_entities

## Partner profile

$partner_profile

## Data gaps

$gaps

## Write

Return a JSON object with exactly these keys:

- `title` — 2-6 words, a glanceable header for the current shape of the day.
- `one_sentence` — one sentence that can open chat or a mobile surface.
- `full_details` — up to 8 sentences describing the current situation shown by the
  evidence. Lead with what matters now; use fewer sentences when little is known.
- `needs_you` — 0-7 ranked action items as strings. Draw them from upcoming
  calendar events needing preparation or explicit unresolved needs in the
  activity evidence. Do not infer urgency from a name or background detail.

## Voice

Every string value is shown directly to the owner in their own journal.
Address them in second person ("you"/"your") — never in the third person, and
never as "the user" or "the owner." Never write as "I", "we" or "my" — the
software has no voice of its own here, it is describing the owner's day.
Write the way a person would describe
their own day to themselves: plain, direct, specific. Avoid corporate or
bureaucratic phrasing ("aligned with communication standards," "leveraging,"
"initiate protocol for") — say what's actually happening in ordinary words.
Personal and family matters (a kid's first day of school, a doctor's
appointment) are not "professional priorities" — don't force a work frame
onto something that isn't work.

Be concise. Do not greet the owner. Do not include markdown outside string
values. Do not mention that you are using a pre-hook or schema.

Output only the JSON object.
