{

  "description": "Photos, video players, image galleries, or visual media dominating the view, even when displayed inside a browser tab",
  "output": "markdown",
  "extraction": "Extract 1 frame only - video playback content does not benefit from text extraction",
  "importance": "low"

}

# Media Player Text Extraction

Extract text from this media player screenshot (YouTube, streaming services, video players, image galleries).

## Header

`# [Platform - Video/Content Title]`

## Content Focus

- Extract video/content title
- Include creator, channel, or artist name
- Include platform name if identifiable
- Include playback position and duration if visible (e.g., `3:42 / 12:15`)
- Include visible captions or subtitles
- Include playlist or queue context if visible
- Do NOT describe the visual content of the video/image itself

## Quality

- Focus on metadata and text overlays, not visual content
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown.
