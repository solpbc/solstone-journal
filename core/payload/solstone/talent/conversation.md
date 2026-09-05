{
  "type": "generate",
  "title": "Conversation Story",
  "description": "Generates a conversation story, topics, and structured commitments, closures, decisions, and relations to merge onto the activity record.",
  "color": "#00796b",
  "schedule": "activity",
  "activities": ["meeting", "call", "messaging", "email"],
  "priority": 20,
  "output": "json",
  "max_output_tokens": 2048,
  "schema": "story.schema.json",
  "hook": {"post": "story"},
  "degradation_check": true,
  "load": {
    "transcripts": true,
    "percepts": true,
    "talents": false
  }
}

$facets

$activity_context

$activity_preamble

# Conversation Story

Write JSON only. No markdown fences. No prose outside the JSON object.

Summarize this conversation as one coherent narrative for the full activity.
Participation and entity extraction already happened upstream. Reuse that context;
do not re-extract people or entities into new structures.

Return exactly this seven-field JSON object:
- `body`: string narrative prose covering what was discussed, what moved, and any commitments.
- `topics`: array of short string tags; use `[]` when there are no durable topics worth preserving.
- `confidence`: float from 0.0 to 1.0.
- `commitments`: array of objects with required string fields `owner`, `action`, `counterparty`, `when`, `context`.
  Example: `{"owner":"Mina","action":"send the revised deck","counterparty":"Ravi","when":"Friday morning","context":"Mina committed to send the deck before the investor follow-up."}`
- `closures`: array of objects with required string fields `owner`, `action`, `counterparty`, `resolution`, `context`. `resolution` must be one of `sent`, `done`, `signed`, `dropped`, `deferred`.
  Example: `{"owner":"Ravi","action":"intro email","counterparty":"Mina","resolution":"sent","context":"Ravi confirmed the intro email already went out during the call."}`
- `decisions`: array of objects with required string fields `owner`, `action`, `context`, plus nullable `counterparty`; emit `null` when there is no counterparty.
  Example: `{"owner":"Team","action":"schedule the launch review for next Tuesday","counterparty":null,"context":"The group agreed to move the review to Tuesday after checking calendars."}`
- `relations`: array of objects with required fields `from`, `to`, `kind`, `note`, `quote`. Use entity NAMES, not ids. `kind` must be one of `works-with`, `works-at`, `reports-to`, `family-of`, `knows`, `uses`, `created`, `other`.
  Example: `{"from":"Mina","to":"Ravi","kind":"works-with","note":"","quote":"Mina and Ravi will co-own the investor follow-up."}`
  Use `[]` unless a relationship is actually evidenced in the content. `note` is required; use `""` when the kind speaks for itself, but explain the relationship when `kind` is `"other"`.

Return `[]` if you do not observe a clear commitment / closure / decision / relation. Better to omit than invent.

Body requirements:
- Write one tight paragraph in chronological order.
- Include 1-3 short verbatim quotes inline only when they sharpen a decision,
  commitment, or disagreement.
- Focus on the actual exchange, not generic meeting boilerplate.
- If the activity mixes channels, unify them into one narrative rather than
  listing separate threads.

Output a single JSON object with all seven required fields: `body`, `topics`, `confidence`, `commitments`, `closures`, `decisions`, and `relations`.
