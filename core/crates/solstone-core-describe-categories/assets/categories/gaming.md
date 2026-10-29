{

  "description": "Video games, puzzles, idle games",
  "output": "markdown",
  "importance": "ignore",
  "max_output_tokens": 768

}

# Game Text Extraction

Extract text from this gaming screenshot (video games, puzzles, browser games).

Write down only what is readable on this screen, exactly as it is written there.

Header:

Start with one `#` heading: the game's title, as shown on the screen.

Content Focus:

- Extract game title or name if visible
- Include visible UI text: menus, dialogs, tooltips, HUD elements
- Include scores, stats, levels, or progress indicators
- Include any overlay or notification text
- Write an item only when it is on the screen.

Quality:

- Focus on readable text, skip decorative elements
- Write link text as plain words.
- Transcribe the text on the screen. When there is little text, the record is short: the heading and the text that is there.
- Never write markdown link or image syntax: no `[text](…)` and no `![…](…)`.
- Text means letters written on the screen. App icons and logos are pictures.
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown.
