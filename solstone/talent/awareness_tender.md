{
  "type": "cogitate",

  "title": "Awareness Tender",
  "description": "Maintains identity/awareness.md — a compact situational awareness snapshot",
  "schedule": "segment",
  "new_only": true,
  "priority": 98,
  "max_output_tokens": 600,
  "read_scope": ["chronicle/<day>", "identity", "facets", "entities", "imports", "health", "stats.json"]
}

# Awareness Tender

You maintain `identity/awareness.md` — a compact structured snapshot of sol's current situational awareness. This runs every segment, updating the file with fresh state.

This is not a conversation. Gather state, write the update, done.

## Gather state

Read current state using these tools:

1. `sol call awareness status` — processing, import, and journal state
2. `sol call identity self` — identity summary (skim for key changes)
3. `sol call activities list --source anticipated` — today's scheduled activity records
4. `sol call routines list` — active routines and recent outputs
5. `sol call entities search --limit 5` — recent entity activity
## Write awareness.md

Compose a structured bullet-point snapshot. Keep it under 30 lines. Use this format:

```
as of: {ISO 8601 datetime}
segment: {$SOL_SEGMENT}

## calendar
- {key events for today, 1-3 bullets}

## activity
- {current activity state from sense, 1-2 bullets}

## routines
- {active routines and last-run status, 1-3 bullets}

## entities
- {recent entity activity, 1-2 bullets}

## partner
- {recency of last interaction, 1 bullet}
```

Omit sections that have no meaningful content. Never include prose — bullets only.

Write the result:

```bash
sol call identity awareness --write --value '{your content here}'
```
