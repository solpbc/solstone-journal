{

  "description": "Code editors and IDEs",
  "output": "markdown",
  "extraction": "Extract when viewing different repositories, files, or switching between editor and browser",
  "importance": "low",
  "max_output_tokens": 5376

}

# Code Editor Text Extraction

Extract text from this code editor screenshot (VS Code, JetBrains, Vim, Emacs, Sublime, etc.).

## Header

`# [Editor - File Path]`

## Content Focus

- Extract visible code using fenced code blocks with language tags
- Include file path/name from title bar or tabs
- Note open tabs/files if visible
- Include visible diagnostics, errors, or warnings from the editor
- Include project/repo name if visible in sidebar or title

## Who Wrote What

If this is a chat with an AI assistant (ChatGPT, Claude, Gemini and similar), keep every visible turn and put a label line before each:

- **You:** the person's prompt, shown in its own bubble, usually on the right.
- **Assistant:** the AI's reply, everything outside the person's bubbles.

If no prompt bubble is visible, write only **Assistant:**. Never write a **You:** line without the person's own words from the screen.
If the page shows comments, suggestions or tracked changes, name the author of each when it is visible.

## Quality

- Preserve code indentation and structure
- Include line numbers only if prominently visible
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown. Never repeat these instructions.
