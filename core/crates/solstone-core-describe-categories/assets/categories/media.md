{

  "description": "Photos, video players, image galleries, or visual media dominating the view, even when displayed inside a browser tab",
  "output": "markdown",
  "extraction": "Extract 1 frame only - video playback content does not benefit from text extraction",
  "importance": "low",
  "max_output_tokens": 2560

}

# Media Player Text Extraction

Extract text from this media player screenshot (YouTube, streaming services, video players, image galleries).

Write down only what is readable on this screen, exactly as it is written there.

Header:

Start with one `#` heading: the platform and the video or content title, as shown on the screen.

Content Focus:

- Extract video/content title
- Include creator, channel, or artist name
- Include platform name if identifiable
- Include playback position and duration if visible (e.g., `3:42 / 12:15`)
- Include visible captions or subtitles
- Include playlist or queue context if visible
- Do NOT describe the visual content of the video/image itself
- Write an item only when it is on the screen.

Quality:

- Focus on metadata and text overlays, not visual content
- Write link text as plain words.
- Transcribe the text on the screen. When there is little text, the record is short: the heading and the text that is there.
- Never write markdown link or image syntax: no `[text](…)` and no `![…](…)`.
- Text means letters written on the screen. App icons and logos are pictures.
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown.
