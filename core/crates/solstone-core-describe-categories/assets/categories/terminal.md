{

  "description": "Command line interfaces, logs, shell",
  "output": "markdown",
  "extraction": "Extract when showing distinctly different content (code diffs vs logs vs documentation)",
  "importance": "low"

}

# Terminal Text Extraction

Extract text from this terminal screenshot (shell sessions, command output, log viewers).

## Header

`# [Shell/App - Working Directory or Context]`

## Content Focus

- Extract commands and their output using fenced code blocks
- Include the shell prompt and working directory if visible
- Include error messages and stack traces in full
- For structured output (tables, JSON, logs), preserve formatting
- Note the terminal application or multiplexer if identifiable (e.g., iTerm2, Alacritty)

## Quality

- Preserve exact command syntax and output formatting
- Use fenced code blocks for all terminal content
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown.
