# Third-Party Vendor Libraries

This directory contains third-party JavaScript libraries used across solstone apps.

## Purpose

The `vendor/` directory provides:
- Centralized location for third-party libraries
- Version tracking and documentation
- Consistent access pattern for all apps
- Local copies for reliability (no CDN dependencies)

## Available Libraries

### marked (v15.0.12)

**Purpose**: Markdown parsing and rendering

**License**: MIT (included in file header)

**Source**: https://github.com/markedjs/marked

**CDN Alternative**: `https://cdn.jsdelivr.net/npm/marked/marked.min.js`

**Usage in App Templates**:
```html
<!-- Using helper function (recommended) -->
<script src="&#123;&#123; vendor_lib('marked') &#125;&#125;"></script>

<!-- Using explicit path -->
<script src="&#123;&#123; url_for('static', filename='vendor/marked/marked.min.js') &#125;&#125;"></script>
```

**Example**:
```javascript
// Basic markdown rendering
const html = marked.parse('# Hello World');

// With options
const html = marked.parse(markdown, {
  breaks: true,      // Convert \n to <br>
  gfm: true,        // GitHub Flavored Markdown
  headerIds: false, // Disable auto-generated header IDs
  mangle: false     // Disable email mangling
});
```

**Currently Used By** (legacy references):
- `solstone/convey/templates/chat.html` (via `solstone/convey/static/marked.min.js`)
- `solstone/convey/templates/facet_detail.html` (via CDN)
- `solstone/convey/templates/agents.html` (via CDN)

### DOMPurify (v3.4.0)

**Purpose**: HTML sanitization for untrusted markdown output (defense against XSS in rendered model-emitted content).

**License**: Apache-2.0 OR MPL-2.0 (dual-licensed; either license can be chosen. Compatible with AGPL-3.0-only via MPL-2.0.)

**Source**: https://github.com/cure53/DOMPurify (v3.4.0 — `dist/purify.min.js`, renamed to `dompurify.min.js`)

**CDN Alternative**: `https://cdn.jsdelivr.net/npm/dompurify@3.4.0/dist/purify.min.js`

**Usage in App Templates**:
```html
<script src="&#123;&#123; vendor_lib('dompurify') &#125;&#125;"></script>
<script>
  const safeHtml = DOMPurify.sanitize(marked.parse(userInput));
</script>
```

**Example**:
```javascript
const dirty = 'Hello <img src=x onerror=alert(1)>';
const clean = DOMPurify.sanitize(marked.parse(dirty));
// clean => '<p>Hello <img src="x"></p>'
```

**Currently Used By**:
- All apps (shell-level include via `solstone/convey/templates/app.html`)

## Adding New Libraries

When adding a new third-party library:

1. **Create subdirectory**: `vendor/{library_name}/`
2. **Add minified file**: Copy production-ready `.min.js` file
3. **Check license**: Ensure license is AGPL-compatible and included
4. **Update this manifest**: Add entry with version, purpose, and usage
5. **Test**: Verify library loads and works in development

## Updating Libraries

When updating a library version:

1. **Replace file** in vendor directory
2. **Update version** in this manifest
3. **Test all apps**: Check apps listed in "Currently Used By"
4. **Commit**: Use message format: `chore: update {library} to v{version}`

## Guidelines

- **Prefer local copies** over CDN for reliability and offline development
- **Use minified versions** for production-ready performance
- **Include licenses** either in file headers or separate LICENSE files
- **Document usage patterns** in this manifest
- **Track versions** to enable security updates
- **One library per subdirectory** for clean organization
