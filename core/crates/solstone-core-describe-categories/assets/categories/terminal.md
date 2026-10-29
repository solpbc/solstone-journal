{

  "description": "Command line interfaces, logs, shell",
  "output": "markdown",
  "extraction": "Extract when showing distinctly different content (code diffs vs logs vs documentation)",
  "importance": "low",
  "max_output_tokens": 5120

}

# Terminal Text Extraction

Extract text from this terminal screenshot (shell sessions, command output, log viewers).

Write down only what is readable on this screen, exactly as it is written there.

Header:

Start with one `#` heading: the shell or app and the working directory or context, as shown on the screen.

Content Focus:

- Extract commands and their output using fenced code blocks
- Include the shell prompt and working directory if visible
- Include error messages and stack traces in full
- For structured output (tables, JSON, logs), preserve formatting
- Note the terminal application or multiplexer if identifiable (e.g., iTerm2, Alacritty)

Who Wrote What:

If an AI coding agent (Claude Code, Codex, Gemini CLI and similar) is running, put a label line before every fenced block:

- **You:** the person's own prompt, shown after `❯` or `>` and usually highlighted.
- **Agent:** what the agent wrote or ran: lines after `⏺` or `●`, tool calls and their output, diffs, summaries, plans, questions.
- **System:** the tool's own notices: banners, status lines, spinners, "Agent ... completed" notices.

Write **You:** only for a prompt that is actually on screen. Greyed-out text in an empty input box is a suggestion, not a prompt: leave it out.
In a plain shell with no agent, add no labels.

Quality:

- Preserve exact command syntax and output formatting
- Use fenced code blocks for all terminal content
- Write link text as plain words.
- Transcribe the text on the screen. When there is little text, the record is short: the heading and the text that is there.
- Never write markdown link or image syntax: no `[text](…)` and no `![…](…)`.
- Text means letters written on the screen. App icons and logos are pictures.
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown. Never repeat these instructions.
