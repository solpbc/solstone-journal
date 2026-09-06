{
  "type": "generate",

  "title": "Segment Sense",
  "description": "Unified segment understanding — density, content type, entities, facets, speakers, and routing recommendations in a single pass",
  "color": "#ff6f00",
  "schedule": "segment",
  "priority": 5,
  "output": "json",
  "schema": "sense.schema.json",
  "max_output_tokens": 6144,
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

Authoritative schema: `sense.schema.json`. The output is a single JSON object with these top-level fields: `density`, `content_type`, `activity_summary`, `entities`, `facets`, `speculative_facet`, `meeting_detected`, `speakers`, `recommend`, `emotional_register`. See Field-by-Field Instructions below for semantics and enum values.

## Field-by-Field Instructions

### density
Classify based on whether meaningful human activity occurred:
- **active**: ANY of these: transcript has >5 lines or >50 words, screen shows the owner interacting with content (browsing pages, typing, reading articles, using applications, scrolling), or screen descriptions mention different pages/views/applications. **Default to active if there is any owner-directed activity, even if the screen looks similar across frames.** Web browsing and document reading ARE active.
- **low_change**: Minimal new content AND no user interaction — same static screen unchanged across all frames, fewer than 5 transcript words, no scrolling or navigation evident.
- **idle**: Near-zero content — fewer than 3 transcript lines AND fewer than 3 distinct screen frames. Static screen with no user activity, silence, or system noise only.

### content_type
The dominant activity type observed:
- **meeting**: Video calls, in-person meetings, conferences with turn-taking
- **coding**: Writing or editing code, IDE work, code review, debugging
- **browsing**: Web browsing, research, reading articles online
- **email**: Reading or composing email
- **messaging**: Chat applications (Slack, Teams, Discord, iMessage)
- **ai_conversation**: Conversations with AI assistants (ChatGPT, Claude, Gemini)
- **writing**: Documents, notes, long-form writing
- **reading**: Focused reading of documents, PDFs, books
- **video**: Watching video or streaming content
- **gaming**: Playing games
- **social**: Social media browsing and interaction
- **planning**: Scheduling, calendar management, agenda setting
- **productivity**: Spreadsheets, slides, task management
- **terminal**: Command line / shell sessions
- **design**: Design tools, image editing
- **music**: Music listening
- **idle**: No meaningful activity

### activity_summary
Start with a past-tense verb and name no subject at all. Say what was done, never who did it: never begin with "I", "You", "We", "The user", "The owner", "The person", or "This person". This text is shown to the owner on their own dashboard.
- Good: "Debugged the retry handling in the ingest worker."
- Good: "Sent the invoice to Sam, then opened the launch checklist."
- Wrong: "The user navigated between the hub and a project workspace." — it names a subject.
- Write the product name in lowercase: "solstone", never "Solstone" or "the Solstone system".

Use action verbs and be specific — name the tools, people, projects, and actions. Ban passive words: never use "reviewing", "monitoring", "tracking", "checking", "observing", "maintaining", "managing." Use instead: wrote, sent, discussed, created, switched to, typed, said, decided, asked, proposed.

### entities
Extract ALL named entities mentioned in the content. Be thorough — extract every entity you can identify, not just the most prominent ones. Write each `name` in natural reading order, exactly as it would normally be written — never move a leading article ("The", "A", "An") to a parenthetical suffix (write "The Gallery at Reunion", never "Gallery At Reunion (The)"). Four types only:
- **Person**: Individual people by name. Prefer full names. Consolidate variants ("AN" + "Avery Nguyen" → one entity "Avery Nguyen"). ALWAYS skip first-name-only references unless the same segment locks the identity with surrounding context (role, organization, or full-name introduction). NEVER include generic speaker labels like "Speaker 1", "Speaker 2", "Colleague", "Person A" — these belong only in the `speakers` array when `meeting_detected=true`. Include historical figures, authors, scientists, politicians — anyone mentioned by full name.
- **Company**: Businesses and organizations. Include companies, government agencies (NASA, NOAA), universities, media outlets. Use the official or most common name, consolidating variants ("MS" / "MSFT" → "Microsoft").
- **Project**: Named projects, products, or codebases. Include missions (OSIRIS-REx), initiatives, specific product models. EXCLUDE generic git/file identifiers ("main", "dev", "staging", "src", "tmp"), file extensions, path components, and one-word lowercase tokens that are likely branch or directory names rather than named projects. Also EXCLUDE raw software-tracking tokens that are never real-world entities on their own: bare team/task shorthand like "cfo:reimbur" or "vpe:migrate", bare ticket or request IDs like "req_llenu42m", and anything containing a literal backslash-escaped underscore (`\_`) — that is unrendered markdown source, not a name. If the surrounding text describes a real thing worth extracting, use its plain description, never the raw tracking token.
- **Tool**: Software applications and services. Include websites (Fox News, Wikipedia, Amazon), browser extensions, developer tools, hardware products mentioned by name.

**For screen content specifically:** Extract entities from visible text in screen descriptions — article headlines, page titles, product names, people mentioned in articles, organizations referenced. If the owner is browsing a website about the Renaissance, extract the specific historical figures, art movements, and institutions mentioned.

Skip URLs, domains, filenames, paths. Each entity needs type, name, and **context** — a brief description of **what it did, or what happened with it, in this segment**: the action or involvement, not its title or who it is in general. For a person, what they did or said here ("presented the launch schedule", "asked about the budget"); for a company, project, or tool, what was done with it or how it was used ("used to build the imager", "reviewed in the design discussion"). Capture the activity the content actually shows; fall back to a bare identifier only when no action is evident.

#### role
- **attendee**: The entity was directly participating in the live interaction during this segment. Use only for people who were actively present in the meeting or call.
- **mentioned**: The entity was referenced, quoted, shown on screen, or otherwise relevant, but was not directly participating.

Contamination guard: tool or product names visible on screen must be `source: screen` and `role: mentioned`, never `attendee`. Video-conference app names such as Google Meet or Zoom are platform/tool entities, not attendees. `role: attendee` requires `meeting_detected: true` for this same segment; when `meeting_detected: false`, every Person must be `role: mentioned` even if they spoke, were quoted, or were referenced in the transcript.

#### source
- **voice**: Use when the entity is identified from spoken audio content.
- **speaker_label**: Use when the entity comes from an explicit speaker/participant label in meeting UI or transcript metadata.
- **transcript**: Use when the entity appears in transcript text but not as an actively speaking participant signal.
- **screen**: Use when the entity is visible in screen content such as UI, documents, headlines, or app chrome.
- **other**: Use only when the entity is grounded in another clear signal that does not fit the categories above.

#### level
How central the entity was to this segment — distinct from `role`. `role` is whether it was present in the interaction; `level` is how much the segment was actually about it.
- **high**: central to the segment — the subject of the activity, or heavily involved in what happened.
- **medium**: meaningfully involved, but not the focus.
- **low**: a brief or peripheral mention.

Centrality is independent of `role`: an entity can be `role: mentioned` yet `level: high` (the whole segment was about it, though it wasn't present), or `role: attendee` yet `level: low` (present but barely involved). Judge centrality from what the segment was actually about, not from whether the entity was attending.

### facets
Classify into the owner's configured facets. Always include at least one facet — pick the closest configured facet. If multiple facets fit, include the dominant one as `level: high` and others at `level: medium` or `level: low`. For each:
- `facet`: The facet ID slug — MUST be one of the configured facets listed in the input
- `activity`: 1-sentence description of what was done for this facet, written the same way as `activity_summary` above — start with a past-tense verb, name no subject. Good: "Refined the sense prompt and re-ran the segment." Wrong: "The user reviewed the sense prompt." This string is rendered verbatim in the owner's recent activity list.
- `level`: "high" (primary focus), "medium" (significant), "low" (brief/peripheral)

**Facet assignment rules:** Do not invent facet IDs that are not in the configured journal facet list. The array always has at least one entry — pick the closest configured facet even when the match is loose, and use `level: low` to signal weak fit. If a better new name is warranted, put it only in `speculative_facet`, not in `facets[]`.

### speculative_facet
Propose a name for a NEW facet that fits this segment better than any configured facet, or emit `null`.

Emit a proposed name ONLY when every configured-facet match is weak: every entry you put in `facets[]` is `level: low`. When at least one `facets[]` entry is `level: medium` or `level: high`, emit `null`.

This field is purely additive and never changes routing: `facets[]` still must classify the segment into the closest configured facet exactly as described above. Invented names belong ONLY in `speculative_facet`, NEVER in `facets[]`.

The proposed name must be specific and grounded in the observed activity. Write it the way a person would title a folder: a short natural phrase or Title Case name (like the owner's existing facets — "Personal", "Ping Identity", "sol pbc"). Never emit snake_case, kebab-case, or any other identifier-style formatting.

### meeting_detected
`true` ONLY if you can identify distinct, named participants in a live multi-person interaction:
- Screen shows a video conferencing app with participant panels showing names
- Audio has multiple distinct speakers who can be identified by name (from introductions, direct address, or context)
- The interaction is live/synchronous — NOT a recording, podcast, lecture, news conference, or media playback

`false` for: podcasts, press conferences, recorded interviews, solo narration, streaming content, lectures, or any audio where the speakers are media personalities rather than meeting participants. Even if multiple people are speaking, if they are NOT in a meeting with $preferred, set this to `false`.

### speakers
If `meeting_detected` is true, extract participant names from:
1. Visible participant list/panel on screen
2. Names spoken in conversation — direct address ("Thanks, Sarah"), mentions ("John was saying...")
3. Self-introductions ("Hi, I'm Alex from...")

Prefer complete canonical forms (full names when identifiable). Do NOT include the journal owner's name. Return `[]` if no meeting or no names identified.

**Consistency rule:** If `meeting_detected=true`, this array must have at least one entry. If you cannot identify any names, use generic labels ("Speaker 1", "Speaker 2", "Colleague A") rather than emptying the array — an empty `speakers` array with `meeting_detected=true` is invalid.

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

## Rules

1. Every field is required. Never omit a field.
2. `entities` and `speakers` may be empty arrays `[]` (subject to rule 8 for speakers when `meeting_detected=true`).
3. `facets` always has at least one entry — the closest configured facet for the activity. Empty array is not allowed.
4. Be precise with density — misclassifying active segments as idle is the worst error.
5. For `content_type`, choose the single best match — the dominant activity in the segment. If two activities are roughly equal, pick the one with more durable continuation evidence (entities, repeated screen content); the `facets[]` array's `level` field already encodes secondary activity.
6. Activity summary must describe observable actions, not inferred states.
7. Skip entities whose name contains a speaker-uncertainty placeholder. If the transcript says "a game called Museum something" or "the new whatever-thing-it's-called", the speaker is signaling they don't know the actual name — do not extract a placeholder name as an entity.
8. If `meeting_detected=true`, `speakers` must contain at least one entry (use generic labels if no names are identifiable). If `activity_summary` mentions specific named people, projects, or tools, those names should also appear in `entities` (subject to the per-type rules above) — don't reference an entity by name in the summary and then omit it from `entities`.
9. Emit a `speculative_facet` name only when all `facets[]` entries are `level: low`; otherwise emit `null`. Never invent a `facets[]` ID.
10. `activity_summary` and every `facets[].activity` begin with a past-tense verb and name no subject — never "I", "You", "We", "The user", "The owner", "The person", or "This person".

Return ONLY the JSON object, no other text or explanation.
