{

  "description": "Social platforms with feeds, threads, profiles, posts, comments, or timelines (X, Bluesky, Reddit, Instagram, TikTok, LinkedIn, Mastodon, HN)",
  "output": "markdown",
  "extraction": "Extract when platform, feed context, or visible thread changes",
  "max_output_tokens": 3072

}

# Social Feed Text Extraction

Extract text from this social platform screenshot (X, Bluesky, Reddit, Instagram, TikTok, LinkedIn, Mastodon, Hacker News, etc.).

Write down only what is readable on this screen, exactly as it is written there.

Header:

Start with one `#` heading: the platform and the feed, thread or profile shown, as shown on the screen.

Post Format:

Extract visible posts with author attribution:

```markdown
**@author**: post text
```

Preserve reply and thread structure when visible (nest replies under their parent).

Note focal media briefly in brackets: `[photo: sunset over water]`, `[video: 0:45 clip]`.

Quality:

- Skip navigation, trends/sidebars, ads, cookie banners, and suggested-follow modules
- Preserve post order (top of feed/thread first)
- Write link text as plain words.
- Transcribe the text on the screen. When there is little text, the record is short: the heading and the text that is there.
- Never write markdown link or image syntax: no `[text](…)` and no `![…](…)`.
- Text means letters written on the screen. App icons and logos are pictures.
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown.
