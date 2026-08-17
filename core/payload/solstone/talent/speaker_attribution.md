{
  "type": "generate",

  "title": "Speaker Attribution",
  "description": "Identifies who said what in each transcript segment. Layers 1-3 (owner, structural, acoustic) run computationally via hook; Layer 4 uses contextual LLM analysis for remaining unmatched sentences.",
  "schedule": "segment",
  "priority": 40,
  "output": "json",
  "schema": "speaker_attribution.schema.json",
  "color": "#d84315",
  "hook": {"pre": "speaker_attribution", "post": "speaker_attribution"},
  "load": {"transcripts": true, "talents": {"screen": true}}

}

$segment_preamble

$unmatched_context

# Speaker Attribution — Contextual Identification (Layer 4)

## Context

Layers 1-3 of speaker attribution have already resolved most sentences:
- **Layer 1** identified the journal owner's sentences via voice embedding similarity
- **Layer 2** used structural heuristics (speaker count, meeting metadata) to label non-owner sentences
- **Layer 3** matched remaining sentences against known voiceprints

The unmatched sentences shown above could not be resolved by these methods. Your task is to identify the speaker for each unmatched sentence using contextual clues from the transcript.

## Identification Strategies

1. **Direct naming:** "Hey Jack", "Thanks Sarah", "I'm Mike from..."
2. **Topic expertise:** Match discussion topics to known speaker backgrounds or roles
3. **Conversation flow:** Speakers typically alternate; a response to a known speaker is likely from a different person
4. **Self-reference:** "In my team we..." paired with known organizational affiliations
5. **Address patterns:** Questions directed at someone ("What do you think, Ryan?") followed by responses

## Output Format

Return a JSON array of attributions for unmatched sentences only:

```json
[
  {"sentence_id": 3, "speaker": "Ryan Bennett", "reasoning": "Responds to owner's question about engineering timeline"},
  {"sentence_id": 7, "speaker": "Jack Andersohn", "reasoning": "References 'my team' consistent with Jack's engineering role"}
]
```

Rules:
- Only attribute sentences you can identify with reasonable confidence
- Use full names when possible, matching the known speakers listed above
- Omit sentences you cannot confidently attribute — they will remain unlabeled rather than wrongly labeled
- Return an empty array `[]` if no attributions can be made
- Return ONLY the JSON array, no other text
