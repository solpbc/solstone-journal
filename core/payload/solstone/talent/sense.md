{
  "type": "generate",

  "title": "segment sense",
  "description": "unified segment understanding, covering density, content type, entities, facets, speakers, and routing recommendations in a single pass",
  "color": "#ff6f00",
  "schedule": "segment",
  "priority": 5,
  "output": "json",
  "schema": "sense.schema.json",
  "max_output_tokens": 6912,
  "timeout_s": 480,
  "load": {"transcripts": true, "percepts": true, "talents": false}
}

$facets

$segment_preamble

# Segment Sense

Analyze this recording segment and produce a single structured assessment covering density, content type, activity, entities, facets, speakers, and processing recommendations.

## Task

Read the transcript and screen data. Produce a JSON object with ALL of the following fields.

## Output Schema

Authoritative schema: `sense.schema.json`. The output is a single JSON object with these top-level fields: `density`, `content_type`, `activity_summary`, `entities`, `facets`, `speculative_facet`, `meeting_detected`, `speakers`, `recommend`, `emotional_register`, `topics`, `quotes`. See Field-by-Field Instructions below for semantics and enum values.

## Field-by-Field Instructions

### density
Classify based on whether meaningful human activity occurred:
- **active**: ANY of these: transcript has >5 lines or >50 words, screen shows the owner interacting with content (browsing pages, typing, reading articles, using applications, scrolling), or screen descriptions mention different pages/views/applications. **Default to active if there is any owner-directed activity, even if the screen looks similar across frames.** Web browsing and document reading ARE active.
- **low_change**: Minimal new content AND no user interaction — same static screen unchanged across all frames, fewer than 5 transcript words, no scrolling or navigation evident.
- **idle**: Near-zero content — fewer than 3 transcript lines AND fewer than 3 distinct screen frames. Static screen with no user activity, silence, or system noise only.

### content_type
First decide whose voices the transcript holds. Speech can come from a show, film, podcast, video or broadcast playing nearby rather than from people present. Signs of media: acted or scripted dialogue, characters calling each other by names that belong to a story, plot and drama (quests, crimes, battles, romance), narration, a host's introduction or sign-off, ads, theme songs, polished delivery from many voices. Signs of people present: everyday logistics, food, children, errands, plans for the day, reactions to each other and to the room. When most of the speech is media, the activity is `video` (or `music` for songs), never `meeting` or `social`, and the media's speakers are not participants. People talking about a show, film, game or ride, planning to watch one, or reacting to one are a conversation: only the media's own voices are media. A recorded announcement in the background does not make a conversation media.

The dominant activity type observed:
- **meeting**: A call or a sit-down meeting in which at least one other participant is named, conferences with turn-taking. A conversation in which no one is named is `social`, never `meeting`. A show, film, podcast, video or broadcast playing nearby is never a meeting, even when its characters or hosts name each other
- **coding**: Writing or editing code, IDE work, code review, debugging
- **browsing**: Web browsing, research, reading articles online
- **email**: Reading or composing email
- **messaging**: Chat applications (Slack, Teams, Discord, iMessage)
- **ai_conversation**: Conversations with AI assistants (ChatGPT, Claude, Gemini)
- **writing**: Documents, notes, long-form writing
- **reading**: Focused reading of documents, PDFs, books
- **video**: Watching video or streaming content in front of the owner (a video playing in a background tab is not `video`), including a show, film, podcast, video or broadcast playing nearby whose voices make up most of the speech, even when people present remark on it
- **gaming**: Playing games
- **social**: Social media browsing and interaction; casual conversation in person, small talk, talk during an errand. Voices from a show, film, podcast, video or broadcast playing nearby are not a conversation. When people present talk with each other while media plays in the background, their conversation is the activity
- **planning**: Scheduling, calendar management, agenda setting
- **productivity**: Spreadsheets, slides, task management
- **terminal**: Command line / shell sessions
- **design**: Design tools, image editing
- **music**: Music listening
- **idle**: No meaningful activity

### activity_summary
The first word is a past-tense verb. Leave out who did it when that is the owner: no name, pronoun or noun comes before the verb. Preserve who did what inside the sentence. When speech reports another person's action, keep that person's name and the fact that it was reported: "Reported that Maya checked the irrigation pump." Do not turn it into "Checked the irrigation pump." An unnamed voice does not establish that the owner spoke or performed the described action. This text is shown to the owner on their own dashboard. Write about what the owner did and what the content says, never about the software itself taking anything in.
- Shape: "[past-tense verb] [the specific thing] [with whom, or using what]". The brackets stand for the segment's own content; never write the brackets or these words.
$imported_recording_rule
- A document, receipt or image establishes its contents, not that the owner bought the items, calculated a total or carried out a plan. Keep visible facts distinct from reported needs and completed actions. Do not infer an action from a document's presence.
- Attribute each action separately. A person named in one statement is not automatically the speaker or actor in the next statement. When an unnamed speaker says that Priya repaired a fence, then says paint is needed, summarize both facts without saying Priya requested the paint: "Reported that Priya repaired the fence; also reported a need for paint." An unknown speaker stays unknown; "we need" does not name who needs or requested something.
- When the owner works with an assistant or agent, the owner's part is what they typed, asked for, approved, declined or read. What the assistant or agent wrote, ran, changed or reported is its work: say the owner asked for it or read it, never that the owner did it.
- Browser content shows the pages in front of the owner first. Pages listed under "Open, not in front of a focused window" were only left open: when they matter, mention them as context ("with [page] open in a background tab"). What the owner read is the text shown from pages in front, and what they watched is a video playing in front. When no page in front sent anything, say what was left open.
- What a page or video says or shows belongs to the page or video. Write what the owner read or watched and what it was about; the people, steps and events in it are not the owner's actions or experience.
- Write the product name in lowercase: "solstone", never "Solstone" or "the Solstone system".

Lead with the main thing the owner was doing and what it was about, naming the specific project, document, person or tool. Leave out window switching and navigation unless that was the point. Prefer plain verbs: wrote, sent, asked, read, discussed, decided, approved, proposed.

### entities
Extract ALL named entities mentioned in the content. Be thorough — extract every entity you can identify, not just the most prominent ones. Write each `name` in natural reading order, exactly as it would normally be written — never move a leading article ("The", "A", "An") to a parenthetical suffix (write "The Gallery at Reunion", never "Gallery At Reunion (The)"). Four types only:
- **Person**: Individual human beings by name. Prefer full names. Consolidate variants ("AN" + "Avery Nguyen" → one entity "Avery Nguyen"). ALWAYS skip first-name-only references unless the same segment locks the identity with surrounding context (role, organization, or full-name introduction). NEVER include generic speaker labels like "Speaker 1", "Speaker 2", "Colleague", "Person A" — these belong only in the `speakers` array when `meeting_detected=true`. Include historical figures, authors, scientists, politicians — anyone mentioned by full name. A name is a Person only when it belongs to one human being. An AI assistant, model or coding agent is a Tool, even with a human name or when it writes as "I". A job title, an office or a role word standing in for someone is not an entity. A computer, server or device is never a Person. A username or handle is a Person only when the content ties it to a named human; then write that human's name. People in a story, show, film or game are characters, not people, and that includes the voices and names in a show or film playing nearby: never list a character as a Person. $preferred, the journal's owner, is in every segment: never list them as an entity.
- **Company**: Businesses and organizations. Include companies, government agencies (NASA, NOAA), universities, media outlets. Use the official or most common name, consolidating variants ("MS" / "MSFT" → "Microsoft").
- **Project**: Named projects, products, or codebases. Include missions (OSIRIS-REx), initiatives, specific product models. EXCLUDE generic git/file identifiers ("main", "dev", "staging", "src", "tmp"), file extensions, path components, and one-word lowercase tokens that are likely branch or directory names rather than named projects. Also EXCLUDE raw software-tracking tokens that are never real-world entities on their own: bare team/task shorthand like "ops:reimbur" or "eng:migrate", bare ticket or request IDs like "req_k3v9x2pq", and anything containing a literal backslash-escaped underscore (`\_`) — that is unrendered markdown source, not a name. If the surrounding text describes a real thing worth extracting, use its plain description, never the raw tracking token.
- **Tool**: Software applications and services. Include websites (Fox News, Wikipedia, Amazon), browser extensions, developer tools, hardware products mentioned by name.

**For screen content specifically:** Extract entities from visible text in screen descriptions — article headlines, page titles, product names, people mentioned in articles, organizations referenced. If the owner is browsing a website about the Renaissance, extract the specific historical figures, art movements, and institutions mentioned.

Skip URLs, domains, filenames, paths. Each entity needs type, name, and **context** — a brief description of **what it did, or what happened with it, in this segment**: the action or involvement, not its title or who it is in general. For a person, what they did or said here; for a company, project, or tool, what was done with it or how it was used, in this segment's own words. Capture the activity the content actually shows; fall back to a bare identifier only when no action is evident.

#### role
- **attendee**: The entity was directly participating in the live interaction during this segment. Use only for people who were actively present in the meeting or call.
- **mentioned**: The entity was referenced, quoted, shown on screen, or otherwise relevant, but was not directly participating.

Contamination guard: tool or product names visible on screen must be `source: screen` and `role: mentioned`, never `attendee`. Video-conference app names such as Google Meet or Zoom are platform/tool entities, not attendees. `role: attendee` requires `meeting_detected: true` for this same segment; when `meeting_detected: false`, every Person must be `role: mentioned` even if they spoke, were quoted, or were referenced in the transcript.

#### source
- **voice**: Use for a person whose own voice is in the speech: a transcript line is theirs, or they take part in the conversation.
- **speaker_label**: Use when the entity comes from an explicit speaker/participant label in meeting UI or transcript metadata.
- **transcript**: Use when the entity is named or talked about in the speech, for a person when nothing shows they spoke.
- **screen**: Use when the entity is visible in screen content such as UI, documents, headlines, or app chrome.
- **other**: Use only when the entity is grounded in another clear signal that does not fit the categories above.

#### level
How central the entity was to this segment — distinct from `role`. `role` is whether it was present in the interaction; `level` is how much the segment was actually about it.
- **high**: central to the segment — the subject of the activity, or heavily involved in what happened.
- **medium**: meaningfully involved, but not the focus.
- **low**: a brief or peripheral mention.

Centrality is independent of `role`: an entity can be `role: mentioned` yet `level: high` (the whole segment was about it, though it wasn't present), or `role: attendee` yet `level: low` (present but barely involved). Judge centrality from what the segment was actually about, not from whether the entity was attending.

### topics
What this segment was about, as short phrases someone would type to find it again later: the plans, problems, decisions under discussion, documents, events, places, purchases and numbers that came up, each with enough words to be specific ("Q3 pricing page", "flight to Barcelona in November", "index rebuild taking six hours"). Use the words the content itself uses. Name subjects, not outcomes: never write that something was decided, agreed, promised or done. Up to 12, most central first. A topic may include a name when the pairing matters ("Priya's launch deck"), but do not list names on their own: they belong in `entities`. Empty array when the segment is idle or has nothing to name.

### quotes
Up to 8 short passages of spoken words from the transcript that someone would most want to find again: plans, agreements, requests, offers, dates, amounts and opinions. Copy each `text` exactly from the transcript, 3 to 30 words, without fixing, shortening, summarizing or completing it. Copy only the spoken words: leave out emotion labels, annotations in parentheses or asterisks, timestamps and speaker prefixes. Never quote screen text; what was on screen belongs in `topics`. `speaker` is the label the transcript gives that line ("Speaker 2", a name the transcript itself shows, or the role an AI conversation gives the line); write "unknown" when the line has no label. Never guess a name, and never write that the owner said it unless the transcript labels the line that way. Do not judge whether anything was done. Empty array when nothing was said, and always empty when the segment has no transcript.

### facets
Classify into the owner's configured facets. Always include at least one facet — pick the closest configured facet. If multiple facets fit, include the dominant one as `level: high` and others at `level: medium` or `level: low`. For each:
- `facet`: The facet ID slug — MUST be one of the configured facets listed in the input
- `activity`: 1-sentence description of what was done for this facet, written the same way as `activity_summary` above. The first word is a past-tense verb; preserve named people, reported actions and document contents. Good: "Reported that Maya checked the irrigation pump." Wrong: "Checked the irrigation pump." when only Maya's action was reported. This string is rendered verbatim in the owner's recent activity list.
- `level`: "high" (primary focus), "medium" (significant), "low" (brief/peripheral)

**Facet assignment rules:** Do not invent facet IDs that are not in the configured journal facet list. The array always has at least one entry — pick the closest configured facet even when the match is loose, and use `level: low` to signal weak fit. If a better new name is warranted, put it only in `speculative_facet`, not in `facets[]`.

### speculative_facet
Propose a name for a NEW facet that fits this segment better than any configured facet, or emit `null`.

Emit a proposed name ONLY when every configured-facet match is weak: every entry you put in `facets[]` is `level: low`. When at least one `facets[]` entry is `level: medium` or `level: high`, emit `null`.

This field is purely additive and never changes routing: `facets[]` still must classify the segment into the closest configured facet exactly as described above. Invented names belong ONLY in `speculative_facet`, NEVER in `facets[]`.

The proposed name must be specific and grounded in the observed activity. Write it the way a person would title a folder: a short natural phrase or Title Case name (like the owner's existing facet names listed above). Never emit snake_case, kebab-case, or any other identifier-style formatting.

### meeting_detected
`true` only when all three hold:
- it is a live conversation happening during the segment (a call or an in-person conversation), not a recording, podcast, lecture, broadcast or other media;
- $preferred takes part in it;
- at least one other participant is identified by name: on a participant list, by introducing themselves, or by being addressed by name.

Talk among people present is live even when it is about a show or one plays in the background. Otherwise `false`. If you could not write at least one participant's name into `speakers`, this is `false`. A short exchange, small talk with someone unnamed, an errand or purchase, overheard speech and a single voice are all `false`. When unsure, `false`.

### speakers
If `meeting_detected` is true, extract participant names from:
1. Visible participant list/panel on screen
2. Names of people taking part in the conversation, from direct address ("Thanks, Sarah"); someone only talked about is not a participant
3. Self-introductions ("Hi, I'm Alex from...")

Prefer complete canonical forms (full names when identifiable). Do NOT include the journal owner's name. Return `[]` if no meeting or no names identified.

**Consistency rule:** a meeting always has a named participant, so when `meeting_detected=true` this array lists at least one name. When it is `false`, the array is `[]`.

### recommend
Processing recommendations for downstream agents:
- **screen_record**: `true` if density is "active" AND there is meaningful screen content worth documenting (not just a static/repetitive screen)
- **speaker_attribution**: `true` if `meeting_detected` is true AND there are multiple speakers to attribute

### emotional_register
The observable emotional tone of the segment based on conversation tone, speech patterns, and behavioral signals — not inferred feelings. Choose the single best match:
- **high_energy**: Fast-paced, enthusiastic, productive momentum
- **tense**: Conflict, disagreement, pressure, frustration evident in tone or content
- **focused**: Quiet concentration, deep work, minimal interruption
- **collaborative**: Engaged multi-person work, building on each other's ideas
- **flat**: Low energy, going through motions, no strong signal either way
- **celebratory**: Wins acknowledged, positive outcomes, shared excitement
- **strained**: Fatigue, overload, pushing through difficulty
- **neutral**: No clear emotional register observable — use this as the default when the segment doesn't carry detectable emotional tone

A segment that holds only body data, such as a workout or a set of readings, is **neutral**: its numbers are not an emotional signal.

## Rules

1. Every field is required. Never omit a field.
2. `entities` and `speakers` may be empty arrays `[]` (subject to rule 8 for speakers when `meeting_detected=true`).
3. `facets` always has at least one entry — the closest configured facet for the activity. Empty array is not allowed.
4. Be precise with density — misclassifying active segments as idle is the worst error.
5. For `content_type`, choose the single best match — the dominant activity in the segment. If two activities are roughly equal, pick the one with more durable continuation evidence (entities, repeated screen content); the `facets[]` array's `level` field already encodes secondary activity.
6. Activity summary must describe observable actions, not inferred states.
7. Skip entities whose name contains a speaker-uncertainty placeholder. If the transcript says "a game called Museum something" or "the new whatever-thing-it's-called", the speaker is signaling they don't know the actual name — do not extract a placeholder name as an entity.
8. If `meeting_detected=true`, `speakers` lists at least one named participant. If `activity_summary` mentions specific named people, projects, or tools, those names should also appear in `entities` (subject to the per-type rules above) — don't reference an entity by name in the summary and then omit it from `entities`.
9. Emit a `speculative_facet` name only when all `facets[]` entries are `level: low`; otherwise emit `null`. Never invent a `facets[]` ID.
10. `activity_summary` and every `facets[].activity` begin with a past-tense verb, with nothing before it. Preserve named actors inside reported clauses, only for the actions explicitly attributed to them.
11. Every `quotes[].text` appears word for word in the transcript.
12. `topics` and `quotes` never repeat each other; a quote is the words, a topic is the subject.

Return ONLY the JSON object, no other text or explanation.
