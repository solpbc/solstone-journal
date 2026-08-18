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
7. Confirm the `selectedFacet` cookie is set to the new slug.
8. Return to `/app/settings#facets` and confirm the facet appears in the all-facets list with a link to `/app/settings/facets/<slug>`.

## Needs You Chat Path

1. Ensure `/app/home/api/pulse` returns `needs_you_items`, where each item has exactly `text`, `kind`, and `payload`.
2. For a deterministic sandbox check, seed `identity/pulse.md` with a current-day `updated` frontmatter value and a `## needs you` bullet.
3. Open `/app/home/`.
4. Confirm the Needs You tile renders with `data-needs-you-item`.
5. Click the tile.
6. Confirm the browser lands on `/app/chat/<today>`.
7. Confirm the chat input value is the editable starter prompt, for example `let's dig into Review the launch checklist`.
8. Submit the prompt unchanged.
9. Confirm the stored `owner_message` includes:

```json
{"kind": "needs_you", "item_text": "Review the launch checklist"}
```

10. Confirm editing the prompt before submit omits `source`.

## Verification Commands

Run:

```sh
make test-app APP=settings

# Final-tree gate before merge or release
make ci
```

Use `make verify-api` when API baseline coverage is being audited for this route set.
