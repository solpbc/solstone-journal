{
  "type": "generate",
  "title": "Participation",
  "description": "Consolidates per-segment Sense entity drafts into a structured per-activity participation list.",
  "hook": {"post": "participation"},
  "schedule": "activity",
  "activities": ["*"],
  "priority": 10,
  "output": "json",
  "schema": "participation.schema.json",
  "max_output_tokens": 12288,
  "timeout_s": 480,
  "load": {
    "transcripts": true,
    "percepts": true,
    "talents": {
      "sense": true
    }
  }
}

$facets

$activity_context

$activity_preamble

# Participation Consolidation

You are a quality-judge consolidating per-segment Sense drafts of entities into a single per-activity `participation` list. The loaded `sense.md` snippets provide entity candidates gathered across the activity span. Deduplicate name variants, preserve the strongest role/source signal, and keep only grounded entities that genuinely participated in or were mentioned during this activity.

## Output Schema

```json
{
  "participation": [
    {
      "name": "Full Name",
      "role": "attendee|mentioned",
      "source": "voice|speaker_label|transcript|screen|other",
      "confidence": 0.0,
      "context": "Short explanation of why this entity belongs in the activity",
      "entity_id": null
    }
  ],
  "participation_confidence": 0.0
}
```

`entity_id` must always be `null`; the post-hook resolves it after generation.

## Rules

1. Exclude the journal owner.
2. Never mark someone `role: attendee` in a non-meeting activity.
3. No fabrication — if you didn't see them, don't list them.
4. Empty `participation: []` when no entities were involved.
5. Confidence is subjective but should reflect signal strength (`voice` > `speaker_label` > `transcript` > `screen`).
6. Dedupe variants (e.g., "JB" and "John B." → one entry with the richer name).

Return only the JSON object with `participation` and optional `participation_confidence`.
