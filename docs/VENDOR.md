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

**Currently Used By**:
- `AppServices.renderMarkdown` at `core/crates/solstone-core-convey-shell/assets/static/app.js` — part of the shared client runtime used across apps (home, transcripts, import, news)

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

### Leaflet (v1.9.4)

**Purpose**: The map app's renderer: pan, zoom, scale, and vector layers for borders, towns and location records. No tile layer is ever added, so nothing is fetched off the journal.

**License**: BSD-2-Clause (`vendor/leaflet/LICENSE`)

**Source**: https://registry.npmjs.org/leaflet/-/leaflet-1.9.4.tgz (`dist/leaflet.js`, `dist/leaflet.css`, `LICENSE`, unmodified; tarball SHA-256 `84c65a256e50657896f54c33bd857b6849ebe94c817803be818bf32a3dde0b77`)

**Usage**: the map workspace loads `/static/vendor/leaflet/leaflet.css` and `/static/vendor/leaflet/leaflet.js`. Use only vector layers (`L.polyline`, `L.circle`, `L.circleMarker`); a marker's default icon and the layers control would request images Leaflet expects beside its CSS, which are not shipped.

**Currently Used By**:
- `core/crates/solstone-core-map-web/assets/workspace.html`

### h3-js (v4.5.0)

**Purpose**: The month map's location grid. Stops are binned in the browser, and the page draws each cell from the library's own boundary. Nothing is fetched off the journal, and the cell ids are not stored.

**License**: Apache-2.0 (`vendor/h3/LICENSE`, notice `vendor/h3/NOTICE`)

**Source**: https://registry.npmjs.org/h3-js/-/h3-js-4.5.0.tgz (`dist/h3-js.umd.js`, `LICENSE`, `NOTICE`, unmodified; tarball SHA-256 `f2819d14956bebc98388d25becb84711d0c464c0ef92ffb63e804b01121a6d6d`)

**Usage**: the map workspace loads `/static/vendor/h3/h3-js.umd.js`.

**Currently Used By**:
- `core/crates/solstone-core-map-web/assets/workspace.html`

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
