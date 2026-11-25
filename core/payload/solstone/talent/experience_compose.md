{
  "type": "generate",

  "title": "experience compose",
  "description": "composes a share-ready third-person narrative and chronological moments for one experience",
  "color": "#3730a3",
  "output": "json",
  "schema": "experience_compose.schema.json",
  "hook": {"pre": "experience_compose", "post": "experience_compose"},
  "load": {"transcripts": false, "percepts": false, "talents": false},
  "temperature": 0.2,
  "max_output_tokens": 3072
}

# Experience Composition

Given a single detected experience's constituent activities, corroborated people, media captions, and per-segment timeline excerpts, compose a crisp, share-ready third-person experience record.

## Mandatory Voice, Faithfulness, and Filtering Rules

1. **Third-person narrative voice:** Always write in third person using the exact owner name provided and third-person pronouns. Never use unquoted second-person pronouns (`you`, `your`, `yours`, `yourself`) or unquoted first-person narrator pronouns (`I`, `my`, `me`, `we`, `our`) anywhere in `title`, `summary`, or `moments`.
2. **No speaker placeholders:** Never emit literal `Speaker 1`, `Speaker 2`, or `Speaker N` tokens. Refer naturally to unnamed people as `colleagues`, `friends`, or `attendees` when no corroborated name is provided.
3. **No verbatim transcript dumps:** Never copy verbatim multi-speaker dialogues or `>` blockquotes. Synthesize cleanly in third-person prose.
4. **Distinct heading:** `title` must be a concise, specific event heading (noun phrase or short third-person headline, ~4-10 words) and must never repeat or duplicate `summary`'s opening sentence.
5. **Filter out noise and concurrent screen bleed completely:**
   - Completely omit unrelated trailing side-tasks tacked onto an activity's text. Do not paraphrase them as "minor errands" or "administrative matters" — leave them out completely.
   - Completely omit concurrent screen or desktop bleed where a screen or terminal entry erroneously attaches unrelated local desktop text onto live conversation or call participants. Any entry marked `[secondary screen context — ignore unrelated desktop text]` is background context only; trust the live conversation or audio ground truth.
   - Never leak raw internal channel tokens (`obsidian`, `import.image`, `tmux`, `ios_screenshots`, `ios_mobile`) into prose; use natural words such as `notes`, `photo`, `terminal`, `screenshot`, `messages`, or `browser`.
6. **Grounded physical locations only:** `locations` must list only physical coarse places where the owner actually was during this episode, explicitly named in the included activity text, timeline excerpts, or image captions (0-4 items). Never borrow venue names from facet names, dropped bleed, or another person's destination, and never include street house numbers, full street addresses, passwords, API keys, gate/door/PIN codes, or order/tracking numbers.
7. **Strict factual grounding and group-action restraint:** State only facts supported by the provided constituent activities, timeline excerpts, and media captions. Keep exact relative days and dates intact. Never conflate distinct people's actions: when the source describes a group arriving, leaving, speaking, or moving together, do not attribute the group's action solely to the owner. Never claim the owner entered a room or venue if the source says they stayed outside or turned back. Never speculate about motives.
8. **Storyless rows (`[storyless-fallback]`):** Synthesize cleanly from descriptions, topics, and image captions without boilerplate or apologizing for missing narrative text.
9. **Internal references:**
   - `people_refs`: Include only exact `entity_id` strings from the corroborated people list who genuinely participated in the episode. Never include the owner's `entity_id` or invent IDs.
   - `activity_handles`: Copy only exact `ACT#` handles from the input.
   - `segment_refs`: Copy only exact `YYYYMMDD/stream/HHMMSS_LEN` strings listed in the input. Never invent or modify timestamps.
   - `media_refs`: Copy only exact `media_ref` strings from the input's media items when relevant to that moment; if no `media_ref=` lines appear, output `[]`.

## Structure (`moments[]`)

- `title`: Concise, natural heading (~4-10 words, distinct from `summary`'s first sentence).
- `summary`: 2-3 cohesive third-person sentences summarizing the whole episode cleanly from start to finish with unrelated side-tasks and bleed omitted.
- `moments`: A flat, chronological list of **1 to 5** distinct narrative moments, each with `id` (`"m1"`, `"m2"`, ...), `started_at` and `ended_at` in `YYYY-MM-DD HH:MM` strictly within the time span of its referenced `segment_refs` and `activity_handles`, a concise 1-2 sentence third-person `summary`, `activity_handles`, `segment_refs`, and `media_refs`:
  - Cover every major stage across the episode's span without dropping the opening or closing stage.
  - Collapse fragmented or overlapping cross-channel activities covering one meeting or outing into 2-4 non-redundant moments citing multiple `activity_handles`.
  - Expand a long multi-segment activity into 2-5 chronological moments anchored with exact `segment_refs`.
  - Anchor co-present photos or screenshots on the relevant moment via exact `media_refs`.
- `people_refs`: Exact corroborated `entity_id` list (owner excluded).
- `locations`: Coarse physical places explicitly grounded in the included activities (0-4 items).

## Episode Packet

$experience_packet

## Output

Return only a JSON object matching the schema. Do not wrap in a markdown fence.
