{
  "type": "generate",
  "title": "Entity Detection",
  "description": "Per-segment, per-entity facet-relevance judgment feeding the living detection substrate",
  "color": "#00695c",
  "schedule": "segment",
  "priority": 15,
  "thinking_budget": 2048,
  "max_output_tokens": 1024,
  "output": "json",
  "schema": "detection.schema.json",
  "hook": {"pre": "entities:detection", "post": "entities:detection"},
  "load": {"transcripts": false, "percepts": false, "talents": false}
}

## Your job

You keep a running daily log of who and what genuinely mattered to the journal owner today, organized by the facets — the areas — of their life. Below is one moment from today, what's already been logged, and what was noticed just now. Update the log.

## What's worth logging — the principle

Log something for **what it did in this moment, not what kind of thing it is.** One test, applied the same way to a person, a company, a project, or a tool: *did it genuinely take part in what happened here, or get worked on?*

A few angles on the same idea:

- **Involvement, not prominence.** Someone who spoke, decided, or was worked with counts; someone merely name-dropped does not — however important they are in general. A tool the work was *about* counts; a tool just running in the background while other work happened does not.
- **Being part of an interaction is taking part.** A named person who was in the conversation, meeting, call, or briefing counts — log them even if they're introduced by their title or role rather than a specific action. (Someone merely *referenced* during it, but not in it, is still just a mention.)
- **The moment is the evidence.** Judge from what the context actually shows happening — not from how official, familiar, or technical a name looks. A stray code, an app that was simply open, a place passed through: if the context shows no real part in the activity, it's scenery, not a participant.
- **When the moment is thin, the log is short.** If you're unsure something truly took part, leave it out. Logging nothing is a perfectly good answer.
- **Centrality is a prior, not a rule.** Where the packet notes how central something was — central to the moment, meaningfully involved, or a passing mention — let it tilt a genuine borderline call. It never overrides the involvement test above: something only name-dropped stays out even if it loomed large, and something that truly took part stays in even if it was peripheral.

## The rules

1. **Log only what genuinely took part** in this moment — real participation, or being the thing worked on. Decide from the context, not from the name or what kind of thing it is.
2. **Leave out the scenery** — background apps, devices, stray identifiers, and anything mentioned only in passing or carried in from an unrelated area.
3. **One entry per thing.** Choose the single facet — among those active in this moment — where it most belongs; never log the same thing under more than one facet.
4. **Write its day so far.** Give one short, concrete line of what it did across today in that facet, folding what just happened into anything already logged — about what it *did*, not what it is.

## What you're given

$detection_packet

## What to return

A single JSON object: `{"detections": [ ... ]}`. Each entry has exactly: `name` (exactly as written above), `facet` (one of those active in this moment), `description` (the updated full-day summary). Include only what's worth logging, each at most once. If nothing in this moment genuinely took part, return an empty list.
