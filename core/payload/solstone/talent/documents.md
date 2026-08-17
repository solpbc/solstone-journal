{
  "type": "generate",

  "title": "Document Analysis",
  "description": "Extracts structured intelligence from imported documents",
  "color": "#5c6bc0",
  "schedule": "segment",
  "priority": 10,
  "hook": {"pre": "documents"},
  "thinking_budget": 8192,
  "max_output_tokens": 8192,
  "output": "json",
  "schema": "documents.schema.json",
  "degradation_check": true,
  "load": {"transcripts": true, "percepts": false, "talents": false}

}

$segment_preamble

Analyze the imported document and extract structured intelligence from it.

Return JSON only. No markdown fences. No prose outside the JSON object.

Use only what the document explicitly states; never infer or assume anything not written. Use exact names, titles, and terms as they appear in the document.

Return exactly this seven-field JSON object:

- `overview`: prose string with document type, title, date, parties, and purpose in 2-3 sentences. If not specified, use the exact phrase "Not specified in this document".
- `parties`: array of objects for every named person or entity and their role. Use `[]` when none are specified. Distinguish primary appointees from successor or contingent appointees with `appointment_tier`. When paraphrasing a legal or formal role, preserve the formal term in `formal_term`, for example `Personal Representative`. Use `""` for required string fields that do not apply.
- `key_provisions`: array of objects for substantive terms, obligations, rights, distributions, and powers. Use `[]` when none are specified.
- `assets`: array of objects for assets, accounts, property, real estate, or valuables referenced. Use `[]` when none are specified.
- `conditions`: array of objects for conditions that activate provisions, including death, incapacity, dates, or other triggering events. Use `[]` when none are specified.
- `important_dates`: array of date-and-meaning objects for execution dates, effective dates, deadlines, review dates, and other material dates. `date` is free-form text because documents can use phrases like "the third anniversary of the Settlor's death". Use `[]` when none are specified.
- `summary`: plain-language prose string suitable for quick reference. If not specified, use the exact phrase "Not specified in this document".

Do not include entity IDs. Use names and strings exactly as written in the document.
