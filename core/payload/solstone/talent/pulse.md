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

Write the owner's current Pulse: a compact situational read of the day so far.
The pre-hook has already gathered all context. Do not call tools, do not call
the CLI, and do not read or write files. Return only the JSON object matching
the schema.

Lean on the previous pulse for continuity. If nothing materially changed, say
that plainly. If something shifted, name the shift. Notice the emotional texture
of the day when the evidence supports it — a tense meeting after quiet work, a
celebratory call, a long focused stretch — but do not force mood language when
the day is neutral.

## Previous pulse

$previous_pulse

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
- `full_details` — 3-8 sentences describing the shape of the owner's day so far.
  Lead with what matters most right now. Mention upcoming events, active work,
  and meaningful shifts since the previous pulse.
- `needs_you` — 0-7 ranked action items as strings. Draw them from upcoming
  calendar events needing preparation, entity follow-ups, completed activities,
  and anything the narrative makes urgent.

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
