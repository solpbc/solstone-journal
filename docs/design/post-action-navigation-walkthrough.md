# Post-Action Navigation Walkthrough

> Facet detail at `/app/settings/facets/{slug}` shipped in `solstone-core-settings-web`. `make test-app` and Python routes are gone.


Use this recipe on a fresh or sandbox journal to verify the facet-detail and Needs You post-action paths end to end.

## Facet Creation Path

1. Start the app with `make sandbox` or `make dev`.
2. Open `/app/settings#facets`.
3. Create a facet with a clear title, emoji, and color.
4. Confirm the browser lands on `/app/settings/facets/<slug>`.
5. Confirm the detail view shows:
   - `<title> is ready`
   - the emoji and color swatch
   - the value-framing paragraph
   - primary action: `tag entities to <title>`
   - secondary action: `create another facet`
   - tertiary action: `back to settings`
6. Click the primary action and confirm the browser lands on `/app/entities/`.
7. Confirm the Settings workspace keeps its local facet selection in its own URL/query state.
8. Return to `/app/settings#facets` and confirm the facet appears in the all-facets list with a link to `/app/settings/facets/<slug>`.

## Informational Needs You Item Path

Former chat, confirm, attention, and plain-string items now classify as
`kind: "note"`, `disabled: false`, with no `payload.prompt`. The home renderer
gates the clickable affordance on `kind === "route"`; informational items render
as text, not a click into chat. There is no `/app/chat/<today>` landing and no
starter prompt.

Grounding: `classify_needs_you(&attention, &deduped_pulse_needs)` at
`core/crates/solstone-core-home/src/pulse.rs:144` produces `needs_you_items`.
`core/crates/solstone-core-home-web/assets/home.js` renders `role="button"` /
`data-needs-you-item` only when `kind === 'route'`; other kinds are plain,
non-clickable text.

1. Ensure `/app/home/api/pulse` returns `needs_you_items`.
2. For a deterministic sandbox check, seed `identity/pulse.md` with a current-day `updated` frontmatter value and a `## needs you` bullet.
3. Open `/app/home/`.
4. Confirm a `kind: "note"` item renders as plain text (no `role="button"`, no `data-needs-you-item`).
5. Confirm it is not a click into chat: there is no `/app/chat/<today>` landing and no starter prompt.

## Verification Commands

Run:

```sh
make test-app APP=settings

# Final-tree gate before merge or release
make ci
```

Use `make verify-api` when API baseline coverage is being audited for this route set.
