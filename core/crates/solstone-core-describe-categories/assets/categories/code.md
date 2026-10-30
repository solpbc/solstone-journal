{

  "description": "Code editors and IDEs",
  "output": "markdown",
  "extraction": "Extract when viewing different repositories, files, or switching between editor and browser",
  "importance": "low",
  "max_output_tokens": 5376

}

# Code Editor Text Extraction

Extract text from this code editor screenshot (VS Code, JetBrains, Vim, Emacs, Sublime, etc.).

Write down only what is readable on this screen, exactly as it is written there.

Header:

Start with one `#` heading: the editor and the file path, as shown on the screen.

Content Focus:

- Extract visible code using fenced code blocks with language tags
- Include file path/name from title bar or tabs
- Note open tabs/files if visible
- Include visible diagnostics, errors, or warnings from the editor
- Include project/repo name if visible in sidebar or title

Quality:

- Preserve code indentation and structure
- Include line numbers only if prominently visible
- Write link text as plain words.
- Transcribe the text on the screen. When there is little text, the record is short: the heading and the text that is there.
- Never write markdown link or image syntax: no `[text](…)` and no `![…](…)`.
- Text means letters written on the screen. App icons and logos are pictures.
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown. Never repeat these instructions.
