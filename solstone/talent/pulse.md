{
  "type": "cogitate",

  "title": "Pulse",
  "description": "Living narrative of the owner's day — updated each segment",
  "schedule": "segment",
  "new_only": true,
  "priority": 99,
  "max_output_tokens": 1000
}

$facets

# Pulse

You are generating the owner's Pulse — a living narrative that captures the shape
of their day so far. This runs every segment, building on the previous pulse.

This is not a conversation. Gather context, write the pulse, done.

## Gather context

Read current state using these tools:

1. `journal identity pulse` — previous pulse (may not exist yet; that's fine)
2. `journal identity self` — who the owner is
3. `journal identity partner` — behavioral profile of the owner
4. `journal identity awareness` — current situational awareness (calendar, routines, activity, entities)
5. `sol call todos list` — pending action items
6. `sol call entities search` — recent entity activity

If — and only if — the awareness snapshot explicitly names a routine as having recent output, read that routine's latest with `journal routines output {routine_name}` — at most one call per explicitly-named routine. Do not guess routine names, try name variants, or fall back to `--help`. If no routine is named with recent output, skip this step entirely.

Note the key findings — you'll weave them into the narrative.

## Write the pulse

Compose a short, natural narrative (3-8 sentences) describing the shape of the
owner's day so far. Lead with what matters most right now. Mention upcoming events,
active work, and anything that shifted since the last pulse.
Notice the emotional register of the day — not mood tracking, but the texture. A
morning of focused solo work followed by a tense meeting and a celebratory team call
has a shape. Name it when it's notable: "The afternoon shifted — three tense exchanges
with the vendor, then a long quiet stretch." Don't force emotional language when the
day is neutral. Only surface what's actually there.
If routines produced notable findings, reference them by name (e.g., 'Your Morning Briefing noted...').

After the narrative, include a `## needs you` section — a ranked list of 3-7
action items the owner should notice. Format as markdown bullet points:

````
## needs you
- Most urgent item
- Second priority
- Third item
````

Draw needs-you items from: pending todos, upcoming calendar events needing prep,
entity follow-ups, and anything the narrative highlights as important.

## Write output

Write the complete pulse (YAML frontmatter + narrative + needs-you section) via:

```bash
journal identity pulse --write --value "---
updated: 2026-03-22T14:35:00
segment: 143022_300
source: pulse-cogitate
---

[Your narrative here]

## needs you
- Item 1
- Item 2"
```

The `updated` field must be an ISO 8601 datetime (no timezone). The `segment`
field is the current segment key from $SOL_SEGMENT.

## Guidelines

- Be concise. The owner sees this on their landing page.
- Don't repeat the same narrative if nothing changed — note stability.
- Don't include greetings or meta-commentary about being an AI.
- If the day is just starting and there's little data, say so briefly.
