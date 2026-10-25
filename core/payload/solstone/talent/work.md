{
  "type": "generate",
  "title": "Work Story",
  "description": "Generates a work story, topics, confidence, and relations to merge onto the activity record.",
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

Return exactly this four-field JSON object:
- `body`: string narrative prose about the work performed and what changed.
- `topics`: array of short string tags; use `[]` when there are no durable topics worth preserving.
- `confidence`: float from 0.0 to 1.0.
- `relations`: array of objects with required fields `from`, `to`, `kind`, `note`, `quote`. Use entity NAMES, not ids. `kind` must be one of `works-with`, `works-at`, `reports-to`, `family-of`, `knows`, `uses`, `created`, `other`.
  Example: `{"from":"Avery","to":"Queue Worker","kind":"created","note":"","quote":"Avery finished wiring the queue worker into the retry path."}`
  Use `[]` unless a relationship is actually evidenced in the content. `note` is required; use `""` when the kind speaks for itself, but explain the relationship when `kind` is `"other"`.

Return `[]` if you do not observe a clear relation. Better to omit than invent.

Who acted. `from` and `to`, and credit in the body, say who acted:
- `"you"`: the journal owner.
- `"your agent"`: an AI assistant working for you in a terminal, chat or tool, such as a coding agent. What it reports, plans, proposes or finishes is its own, never yours. In an agent's terminal or chat, only what you typed or sent is yours.
- a person's name: someone else, as the source names them.
- `"unknown"`: the source does not show who. Never guess `"you"`.

Seen is not done. Screen text shows what was in front of you: a page, a document, a chat, an agent's terminal. Credit `"you"` only where the source shows you acting: a prompt or message you typed and sent, a command at your own shell, an edit you made, a form you submitted. Otherwise say what you did with it: you read, reviewed, followed along, or asked your agent. Anything an AI wrote or did is your agent's: replies, drafts, plans, summaries, commands it ran, files it changed, commits it made, even when it is written in your voice or tells you what to do. When the source marks who wrote a passage (You, Assistant, Agent, a sender), follow that mark.

Body requirements:
- Write one tight paragraph in chronological order.
- Address the owner directly in second person ("you reviewed...", "you asked your agent to...", "your agent fixed...").
  This text is shown to them in their own journal — never call them "the user,"
  "this person," "the session," or refer to them in the third person.
  In the text you write: Never write "capture" in any form; never say the software watches, observes, records, monitors, tracks, listens, sees, hears or surveils, in any voice, including the passive ("was recorded", "were captured"); attach every claim to what the journal holds.
- Credit work to whoever did it: what your agent did, say your agent did.
- Emphasize concrete progress, investigation, blockers, and outcomes.
- Prefer the actual work performed over UI description.
- Write only what the source shows. When it shows little, one or two sentences is right.
- Never fill a gap: do not name an app, file, person or tool the source does not name, and do not say what a session was for or what came of it unless the source shows it.
- If evidence is partial, keep the confidence honest.

Output a single JSON object with all four required fields: `body`, `topics`, `confidence`, and `relations`.
