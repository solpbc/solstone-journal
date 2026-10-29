{

  "description": "Documents, articles, PDFs, documentation",
  "output": "markdown",
  "extraction": "Extract when viewing a different document, article, or PDF",
  "importance": "high",
  "max_output_tokens": 2560

}

# Document Reading Text Extraction

Extract text from this document screenshot (PDFs, documentation, articles, ebooks, manuals).

Write down only what is readable on this screen, exactly as it is written there.

Header:

Start with one `#` heading: the document's title, or its type when no title is shown, as shown on the screen.

Content Focus:

Extract all visible text, preserving document structure:

- Use `##`, `###` for section headings
- Preserve numbered sections (e.g., "2.3 Configuration")
- Use markdown tables for tabular data
- Use code fences with language tags for code examples

Who Wrote What:

If this is a chat with an AI assistant (ChatGPT, Claude, Gemini and similar), keep every visible turn and put a label line before each:

- **You:** the person's prompt, shown in its own bubble, usually on the right.
- **Assistant:** the AI's reply, everything outside the person's bubbles.

If no prompt bubble is visible, write only **Assistant:**. Never write a **You:** line without the person's own words from the screen.
If the page shows comments, suggestions or tracked changes, name the author of each when it is visible.

Quality:

- Maintain logical reading flow
- Preserve paragraph breaks
- Skip page headers/footers unless meaningful
- Write link text as plain words.
- Transcribe the text on the screen. When there is little text, the record is short: the heading and the text that is there.
- Never write markdown link or image syntax: no `[text](…)` and no `![…](…)`.
- Text means letters written on the screen. App icons and logos are pictures.
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown. Never repeat these instructions.
