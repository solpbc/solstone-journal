{

  "description": "Social platforms with feeds, threads, profiles, posts, comments, or timelines (X, Bluesky, Reddit, Instagram, TikTok, LinkedIn, Mastodon, HN)",
  "output": "markdown",
  "extraction": "Extract when platform, feed context, or visible thread changes"

}

# Social Feed Text Extraction

Extract text from this social platform screenshot (X, Bluesky, Reddit, Instagram, TikTok, LinkedIn, Mastodon, Hacker News, etc.).

## Header

`# [Platform - Feed/Thread/Profile context]`

## Post Format

Extract visible posts with author attribution:

```markdown
**@author**: post text
```

Preserve reply and thread structure when visible (nest replies under their parent).

Note focal media briefly in brackets: `[photo: sunset over water]`, `[video: 0:45 clip]`.

## Quality

- Skip navigation, trends/sidebars, ads, cookie banners, and suggested-follow modules
- Preserve post order (top of feed/thread first)
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown.
