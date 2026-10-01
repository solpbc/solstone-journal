{
  "type": "generate",
  "title": "Work Story",
  "description": "Generates a work story, topics, and structured commitments, closures, decisions, and relations to merge onto the activity record.",
  "color": "#6d4c41",
  "schedule": "activity",
  "activities": ["coding", "browsing", "reading", "terminal", "ai_conversation", "writing", "planning", "design", "productivity"],
  "priority": 20,
  "output": "json",
  "max_output_tokens": 1280,
  "schema": "story.schema.json",
  "hook": {"post": "story"},
  "load": {
    "transcripts": true,
    "percepts": true,
    "talents": false
  }
}

$facets

$activity_context

$activity_preamble

# Work Story

Write JSON only. No markdown fences. No prose outside the JSON object.

Summarize what the owner accomplished, investigated, or worked through during
the activity. Participation and entity extraction already happened upstream.
Use that context; do not re-extract people or entities into new structures.

Return exactly this seven-field JSON object:
- `body`: string narrative prose about the work performed and what changed.
- `topics`: array of short string tags; use `[]` when there are no durable topics worth preserving.
- `confidence`: float from 0.0 to 1.0.
- `commitments`: array of objects with required string fields `owner`, `action`, `counterparty`, `when`, `context`.
  Example: `{"owner":"Avery","action":"post the benchmark results","counterparty":"Priya","when":"after lunch","context":"Avery said the new retry benchmark would be shared once the run completed."}`
- `closures`: array of objects with required string fields `owner`, `action`, `counterparty`, `resolution`, `context`. `resolution` must be one of `sent`, `done`, `signed`, `dropped`, `deferred`.
  Example: `{"owner":"your agent","action":"fix the failing retry test","counterparty":"you","resolution":"done","context":"Your agent reported the fix and a passing test run in its terminal."}`
- `decisions`: array of objects with required string fields `owner`, `action`, `context`, plus nullable `counterparty`; emit `null` when there is no counterparty.
  Example: `{"owner":"you","action":"switch the retry path to queue-backed backoff","counterparty":null,"context":"Your agent proposed two options and you typed \"go with the queue\" to approve the first."}`
- `relations`: array of objects with required fields `from`, `to`, `kind`, `note`, `quote`. Use entity NAMES, not ids. `kind` must be one of `works-with`, `works-at`, `reports-to`, `family-of`, `knows`, `uses`, `created`, `other`.
  Example: `{"from":"Avery","to":"Queue Worker","kind":"created","note":"","quote":"Avery finished wiring the queue worker into the retry path."}`
  Use `[]` unless a relationship is actually evidenced in the content. `note` is required; use `""` when the kind speaks for itself, but explain the relationship when `kind` is `"other"`.

Return `[]` if you do not observe a clear commitment / closure / decision / relation. Better to omit than invent.

Who acted. `owner` says who did, owes, closed or decided the item. It is exactly one of:
- `"you"`: the journal owner.
- `"your agent"`: an AI assistant working for you in a terminal, chat or tool, such as a coding agent. What it reports, plans, proposes or finishes is its own, never yours. In an agent's terminal or chat, only what you typed or sent is yours.
- a person's name: someone else, as the source names them.
- `"unknown"`: the source does not show who. Never guess `"you"`.
`counterparty` uses the same words when the other side is you or your agent.

What counts as a decision. Record one only when the source shows someone deciding: stating a choice, selecting and submitting an option, or approving. A proposal or recommendation (your agent's included), a default or unsubmitted form option, a question and a draft are not decisions. When you approve your agent's proposal, that is your decision, with `owner` `"you"`.

Body requirements:
- Write one tight paragraph in chronological order.
- Address the owner directly in second person ("you wrote...", "you debugged...").
  This text is shown to them in their own journal — never call them "the user,"
  "this person," "the session," or refer to them in the third person.
  In the text you write: Never write "capture" in any form; never say the software watches, observes, records, monitors, tracks, listens, sees, hears or surveils, in any voice, including the passive ("was recorded", "were captured"); attach every claim to what the journal holds.
- Credit work to whoever did it: what your agent did, say your agent did.
- Emphasize concrete progress, investigation, blockers, and outcomes.
- Prefer the actual work performed over UI description.
- If evidence is partial, describe the most defensible story and keep the
  confidence honest.

Output a single JSON object with all seven required fields: `body`, `topics`, `confidence`, `commitments`, `closures`, `decisions`, and `relations`.
