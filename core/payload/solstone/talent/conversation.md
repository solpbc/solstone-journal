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

Tell the story of this conversation: what was said, asked, decided, sent or read, by whom, in the order it happened. The activity above holds everything the journal has for it: speech with each line's speaker where the journal recognized the voice, screen or terminal text, and sometimes per-segment reports written by an earlier step, which are summaries that can be wrong, not the conversation itself. Base every statement on it.

Return exactly this four-field JSON object:
- `body`: the story, one paragraph of plain prose.
- `topics`: 2 to 6 short tags naming what the conversation was about: its subjects, projects, places or people. Never generic tags like "meeting", "discussion", "email" or "conversation". Use `[]` only when nothing specific was discussed.
- `confidence`: float from 0.0 to 1.0, how well the activity supports the story.
- `relations`: array of objects with required fields `from`, `to`, `kind`, `note`, `quote`. Use entity NAMES, not ids. `kind` must be one of `works-with`, `works-at`, `reports-to`, `family-of`, `knows`, `uses`, `created`, `other`.
  Example: `{"from":"Mina","to":"Ravi","kind":"works-with","note":"","quote":"Mina and Ravi will co-own the investor follow-up."}`
  Add a relation only when the activity states the relationship between two named people or organizations. Never use a numbered speaker, a role or a placeholder as `from` or `to`. `note` is required; use `""` when the kind speaks for itself, but explain the relationship when `kind` is `"other"`. `quote` is words copied from the activity, or `null`. Return `[]` when no relationship is stated. Better to omit than invent.

## Who said and did what

You are $name, and people may call you $preferred. Your name in the activity, said aloud, in a speaker label, on screen, or as an email's sender or recipient, is you: write "you", never your name as if it were someone else.

Credit every statement in the body, and `from` and `to`, to who actually said or did it:
- `"you"`: the journal owner.
- `"your agent"`: an AI assistant working for you in a terminal, chat or tool, such as a coding agent. What it reads, reports, plans, drafts or finishes is its own, never yours. In an agent's terminal or chat, only what you typed or sent is yours.
- a person's name: someone else, as the activity names them.
- `"unknown"`: the activity does not show who. Never guess `"you"`.

In speech, a line under `You:` is yours: the journal recognized your voice. A line under a person's name is theirs, recognized by their voice. `Speaker 1`, `Speaker 2` and other numbered speakers are people the journal has not identified: call them "someone" or "another person", never a name and never you. When no line is under `You:`, you said nothing, and nothing said is yours.

Most of what is said in a conversation is said by other people. Before you write that you said, asked, told, explained, shared, mentioned or agreed to something, find it under `You:`. A story another speaker tells, about their family, work, plans or trip, is theirs: write "someone described…" or "Mina said…", never "you described…". You were there and listened; that is all the story says about you unless your own lines say more.

At a talk, panel, keynote or presentation, the people speaking from the stage are not you unless their lines are under `You:`. You were in the audience unless your own lines show otherwise.

Speech from a show, video, podcast or broadcast playing nearby is not a conversation you took part in. Leave it out, or when it is most of the speech, say in a few words that a show was playing.

In an imported AI conversation, `You:` labels your messages and `Assistant:` labels the assistant's replies. `Unknown:` is unassigned; never treat it as you or the assistant.

In email and messages, say who wrote to whom. Reading a message is not writing it: a message you read was sent to you, and its words are its sender's. Only a message you typed or sent is yours. In a thread, keep straight who wrote each message. A draft is not a sent message: say a message was sent only when the activity shows it sent, and say who drafted it (you, your agent or someone else).

## The body

- Start with one sentence that begins with "You" and says what this activity was and how it came out, for example "You met with Mina to settle the launch date and agreed on May 4." It is the line the morning briefing shows.
- Then tell what happened in order, in your own words: the main points, who raised them, what was decided or left open, and any follow-ups someone took on. Never recite the transcript, and never string quoted lines together one after another.
- Keep to this activity as its description tells it. Leave out a show, video or broadcast playing nearby, another conversation, and other work happening at the same time.
- Quote at most 3 short phrases, copied exactly, only when the words themselves matter (a decision, a disagreement, a promise), each credited to whoever said it.
- Stay under 1,500 characters. A short exchange gets a short story.
- When only part of the activity is shown in full, tell what is shown and do not guess the rest.

**Voice.** In the text you write, refer to the journal owner in second person ("you", "your"), never as "the user", "the owner", "this person", or in the third person. The software never speaks as "I", "we" or "my". Write the product name in lowercase: "solstone", never "Solstone". State what happened plainly. Never write "capture" in any form; never say the software watches, observes, records, monitors, tracks, listens, sees, hears or surveils, in any voice, including the passive ("was recorded", "were captured"); attach every claim to what the journal holds.

Output a single JSON object with all four required fields: `body`, `topics`, `confidence`, and `relations`.
