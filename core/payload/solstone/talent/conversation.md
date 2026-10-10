{
  "type": "generate",
  "title": "Conversation Story",
  "description": "Generates a conversation story, topics, confidence, and relations to merge onto the activity record.",
  "color": "#00796b",
  "schedule": "activity",
  "activities": ["meeting", "messaging", "email"],
  "priority": 20,
  "output": "json",
  "max_output_tokens": 1536,
  "schema": "conversation.schema.json",
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

# Conversation Story

Write JSON only. No markdown fences. No prose outside the JSON object.

Summarize this conversation as one coherent narrative for the full activity.
Participation and entity extraction already happened upstream. Reuse that context;
do not re-extract people or entities into new structures.

Return exactly this four-field JSON object:
- `body`: string narrative prose covering what was discussed, what moved, and what happened.
- `topics`: array of short string tags; use `[]` when there are no durable topics worth preserving.
- `confidence`: float from 0.0 to 1.0.
- `relations`: array of objects with required fields `from`, `to`, `kind`, `note`, `quote`. Use entity NAMES, not ids. `kind` must be one of `works-with`, `works-at`, `reports-to`, `family-of`, `knows`, `uses`, `created`, `other`.
  Example: `{"from":"Mina","to":"Ravi","kind":"works-with","note":"","quote":"Mina and Ravi will co-own the investor follow-up."}`
  Use `[]` unless a relationship is actually evidenced in the content. `note` is required; use `""` when the kind speaks for itself, but explain the relationship when `kind` is `"other"`.

Return `[]` if you do not observe a clear relation. Better to omit than invent.

Who acted. `from` and `to`, and credit in the body, say who acted:
- `"you"`: the journal owner.
- `"your agent"`: an AI assistant working for you in a terminal, chat or tool, such as a coding agent. What it reports, plans, proposes or finishes is its own, never yours. In an agent's terminal or chat, only what you typed or sent is yours.
- a person's name: someone else, as the source names them.
- `"unknown"`: the source does not show who. Never guess `"you"`.

Who spoke. In a transcript, a line under `You:` is yours: the journal recognized your voice. A line under a person's name is theirs, recognized by their voice. `Speaker 1`, `Speaker 2` and other unnamed speakers are not identified, and none of them is you. When no line is under `You:`, you said nothing in that transcript, and nothing said in it is yours.

Body requirements:
- Write one tight paragraph in chronological order.
- Include 1-3 short verbatim quotes inline only when they sharpen a disagreement or pivotal point in the exchange.
- Focus on the actual exchange, not generic meeting boilerplate.
- If the activity mixes channels, unify them into one narrative rather than
  listing separate threads.

**Voice.** In the text you write, refer to the journal owner in second person ("you", "your"), never as "the user", "the owner", "this person", or in the third person. The software never speaks as "I", "we" or "my". Write the product name in lowercase: "solstone", never "Solstone". State what happened plainly. Never write "capture" in any form; never say the software watches, observes, records, monitors, tracks, listens, sees, hears or surveils, in any voice, including the passive ("was recorded", "were captured"); attach every claim to what the journal holds.

Output a single JSON object with all four required fields: `body`, `topics`, `confidence`, and `relations`.
