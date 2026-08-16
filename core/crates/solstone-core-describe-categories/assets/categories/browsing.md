{

  "description": "General web browsing, news, shopping, or reference pages without a dominant social feed or media viewer",
  "output": "markdown",
  "extraction": "Extract when visiting distinctly different websites or search results",
  "max_output_tokens": 2048

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

## Quality

- Preserve reading order (main content first)
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown.
