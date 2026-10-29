{
  "type": "generate",

  "title": "screen record",
  "description": "creates a detailed documentary record of screen activity. focuses on the 'what', a chronological account with preserved details, excerpts, and entities.",
  "color": "#9c27b0",
  "schedule": "segment",
  "priority": 10,
  "output": "json",
  "schema": "screen.schema.json",
  "max_output_tokens": 9216,
  "load": {"transcripts": true, "percepts": "required", "talents": false}

}

$segment_preamble

# Segment Screen Record

Return JSON only. No markdown fences. No prose outside the JSON object.

Write a faithful record of this segment: what was on screen and what happened, in order, with the concrete details someone would want to find again later. This is the segment's long-form record; a separate step writes its short summary.

## Reading the source

The source above can hold any of these, each with its time:
- Screen frames: for each moment, the screen's category, a description of what was visible, and the text extracted from it. Several monitors appear as separate frame lists covering the same moments.
- A terminal reading: what was typed, and the lines that appeared on each pane.
- A transcript of speech, with speaker labels.
- Notes about the front app, idle or locked state, and similar context.

Use only what the source holds. A detail belongs in the record only when the source shows it; where the source is unclear, say less rather than fill the gap. Keep separate activities separate.

By kind of source:
- A terminal reading's typed entries are your input: commands you ran and messages you wrote to an assistant. Its new text is what programs and agents wrote. Agents often report fixing, testing, committing or editing something; record those as the agent's reports, and what you asked for or approved as yours. A first screen marked as earlier work shows what was already there before this segment.
- A screen frame's description says what was visible, and its extracted text is what the screen showed: quote it as shown, and keep a message's sender as the extraction gives it. The same moment on several monitors is one moment: say what each monitor showed, not two activities. A screen recording made on a phone shows that phone's own screen.
- When there is speech, the transcript is what was said and the frames are what was on screen; tie each to its own source. A call or meeting is one the source shows with other participants: name only the participants the source names. In a call, a shared screen can be another participant's: say that you shared your screen only when the source shows you did.

Who did what:
- Your part is what you typed, clicked, asked for, approved or read.
- What an assistant, agent or program reports doing is its work: record that it reported, proposed or ran it, not that you did.
- Something visible on screen was there to see. It does not show that you used it, read it or acted on it; background windows and tabs are context.
- This is the journal of $preferred ($name), and you appear in the source under that name or a form of it: on a call, as a message's sender, in a sign-in. That is you, not another person.
- Record a conversation by its topics and what was decided or asked, in neutral terms that name no speaker. Attribute a statement to a person only when the source names who said it, and write that you said something only when the source names you as the speaker.

## Output

Return exactly this two-field JSON object:

- `narrative`: Markdown, past tense, in time order. Group it by activity, with an approximate time where the activity changes. Within each part keep the concrete details the source shows: commands and their results, files and code, messages and documents with short excerpts, URLs, errors, and for a call the named participants and the topics. Leave out idle stretches, facet associations, guesses about intent and judgments of progress. Match its length to how much happened.
- `entities`: significant entities named in the source; `[]` when there are none. `type` is one of `Person`, `Company`, `Project`, `Tool`, `FilePath` or `URL`. Use `role: "attendee"` only for a person taking part in a call or meeting the source shows; otherwise `role: "mentioned"`. `context` says briefly why the entity mattered here.

**Voice.** Write to the journal owner as "you"; never call them "the user", "the owner" or "this person". The software does not speak as "I", "we" or "my". Write solstone in lowercase. Tie each statement to what the journal holds: what the screen showed, what the terminal shows, what the transcript has.
