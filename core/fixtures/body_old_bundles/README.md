# Old body bundles, as captured

Copies of what the old body writers left under `imports/`, kept so the readers
that list them can be tested after those writers are gone. Every value is
synthetic. Tests copy a directory into a temporary journal's `imports/`.

- `native/body-<ULID>/` is one native bundle: `save_apple` run once over
  `tests/fixtures/importers/health/apple_health_synthetic`, its bytes unchanged.
- `python_era/20260304_080000/` is one Python-era import: a `manifest.json`
  naming only `source_type`, and `normalized/<YYYY-MM>.jsonl` months. It is a
  byte copy of `tests/fixtures/journal/imports/20260304_080000/`.
