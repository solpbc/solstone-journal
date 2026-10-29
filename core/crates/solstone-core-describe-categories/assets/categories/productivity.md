{

  "description": "Spreadsheets, slides, document editors, task and issue tracking tools, dashboards, other workplace desktop or web apps and professional tools that are not primarily calendar/scheduling views",
  "output": "markdown",
  "extraction": "Extract when different application or service is shown (e.g. ChatGPT vs Docs vs issue tracker)",
  "importance": "high",
  "max_output_tokens": 3584

}

# Productivity App Text Extraction

Extract text from this productivity screenshot (spreadsheets, slides, document editors, task managers, issue trackers, dashboards, project management tools).

Write down only what is readable on this screen, exactly as it is written there.

Header:

Start with one `#` heading: the app name and the document or view title, as shown on the screen.

Content Focus:

Extract all visible data with appropriate structure:

- **Tables/Spreadsheets**: Use markdown tables, include headers
- **Tasks/Issues**: Include title, status, assignee, and due date if visible
- **Slides**: Use `##` for slide titles, bullets for content
- **Dashboards/Apps**: Preserve labels, values, statuses, and visible hierarchy

Who Wrote What:

If this is a chat with an AI assistant (ChatGPT, Claude, Gemini and similar), keep every visible turn and put a label line before each:

- **You:** the person's prompt, shown in its own bubble, usually on the right.
- **Assistant:** the AI's reply, everything outside the person's bubbles.

If no prompt bubble is visible, write only **Assistant:**. Never write a **You:** line without the person's own words from the screen.
If the page shows comments, suggestions or tracked changes, name the author of each when it is visible.

Quality:

- Preserve data relationships and hierarchy
- Include key metadata (dates, statuses, assignees)
- Write link text as plain words.
- Transcribe the text on the screen. When there is little text, the record is short: the heading and the text that is there.
- Never write markdown link or image syntax: no `[text](…)` and no `![…](…)`.
- Text means letters written on the screen. App icons and logos are pictures.
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown. Never repeat these instructions.
