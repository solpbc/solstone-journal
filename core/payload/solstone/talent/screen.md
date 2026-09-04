{
  "type": "generate",

  "title": "Screen Record",
  "description": "Creates a detailed documentary record of screen activity. Focuses on the 'what' - chronological account with preserved details, excerpts, and entities.",
  "color": "#9c27b0",
  "schedule": "segment",
  "priority": 10,
  "output": "json",
  "schema": "screen.schema.json",
  "max_output_tokens": 8192,
  "load": {"transcripts": true, "percepts": "required", "talents": false}

}

$segment_preamble

# Segment Screen Record

Return JSON only. No markdown fences. No prose outside the JSON object.

Create a detailed documentary record of what occurred on screen during this segment. Your job is to produce a comprehensive, factual account that preserves important details for future reference and review.

The segment data includes audio transcript content and frame-by-frame screen activity with timestamps, monitor names, activity categories, visual descriptions, extracted text, and meeting analysis.

Return exactly this two-field JSON object:

- `narrative`: a Markdown string containing the detailed activity log. Use past tense. Structure it chronologically by time periods or major activity shifts. Include approximate timestamps for transitions. Weave multiple monitors into a coherent timeline. Include key commands and outputs, specific files, functions, or code sections edited, relevant message excerpts, documentation topics reviewed, URLs, file paths, and error messages. For meetings, list participants detected, summarize discussion topics, and note shared screens, slides, or documents. Do not include idle activities, facet associations, interpretation of intent, or progress-state analysis.
- `entities`: array of significant entities encountered. Use `[]` when there are none. Use `type` values `Person`, `Company`, `Project`, `Tool`, `FilePath`, or `URL`. Use `role: "attendee"` only for people visibly participating in a meeting or call; use `role: "mentioned"` for all other entities and non-attendee people. `context` should briefly state why the entity mattered in this screen record.

The rendered record should let someone understand exactly what happened on screen without watching the recording.
