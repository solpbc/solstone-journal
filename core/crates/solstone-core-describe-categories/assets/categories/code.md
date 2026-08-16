{

  "description": "Code editors and IDEs",
  "output": "markdown",
  "extraction": "Extract when viewing different repositories, files, or switching between editor and browser",
  "importance": "low"

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

## Quality

- Preserve code indentation and structure
- Include line numbers only if prominently visible
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown.
