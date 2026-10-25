{

  "description": "General web browsing, news, shopping, or reference pages without a dominant social feed or media viewer",
  "output": "markdown",
  "extraction": "Extract when visiting distinctly different websites or search results",
  "max_output_tokens": 3072

}

# Web Browsing Text Extraction

Extract text from this web browsing screenshot (news, blogs, social media, shopping, general websites).

## Header

`# [Site Name - Page Title]`

## Content Focus

Extract the primary page content. Skip navigation menus, sidebars, ads, cookie banners, and footers.

For articles: use `##` for headlines, preserve paragraph structure.

For social feeds: include username, post content, and engagement counts if visible.

For product pages: include product name, price, and key details.

## Who Wrote What

If this is a chat with an AI assistant (ChatGPT, Claude, Gemini and similar), keep every visible turn and put a label line before each:

- **You:** the person's prompt, shown in its own bubble, usually on the right.
- **Assistant:** the AI's reply, everything outside the person's bubbles.

If no prompt bubble is visible, write only **Assistant:**. Never write a **You:** line without the person's own words from the screen.
If the page shows comments, suggestions or tracked changes, name the author of each when it is visible.

## Quality

- Preserve reading order (main content first)
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown. Never repeat these instructions.
