{

  "description": "Video games, puzzles, idle games",
  "output": "markdown",
  "importance": "ignore"

}

# Game Text Extraction

Extract text from this gaming screenshot (video games, puzzles, browser games).

## Header

`# [Game Title]`

## Content Focus

- Extract game title or name if visible
- Include visible UI text: menus, dialogs, tooltips, HUD elements
- Include scores, stats, levels, or progress indicators
- Include any overlay or notification text

## Quality

- Focus on readable text, skip decorative elements
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown.
