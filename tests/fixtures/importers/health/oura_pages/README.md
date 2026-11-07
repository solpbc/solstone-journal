# Synthetic staged Oura pages

A folder of Oura API v2 pages as the body writer's Oura run reads them
(`solstone-core-import-sources/src/oura_run.rs`): `index.json` lists each page in
fetch order with the SHA-256 of its bytes, and `<endpoint>/<chunk>-<n>.json` is a
page body as Oura returns it, `{"data": [...], "next_token": ...}`. Every value is
invented; no real account, device or health data is here, and never may be.

The pages are hashed, so `.gitattributes` keeps their bytes exactly as written. To
change one, rewrite the page and its `sha256` in `index.json` together.

Where each shape comes from:

- **Oura's published OpenAPI (v2, 1.43):** the required and nullable fields of every
  endpoint, the enums (`sleep.type`, `session.type`, `workout.intensity`,
  `workout.source`, `heartrate.source`), and the units: `workout.distance` in metres,
  `workout.calories` in kilocalories, durations in seconds.
- **Oura's sandbox** (`/v2/sandbox/usercollection/*`, its fake data): the key order and
  the nulls Oura actually sends. In particular, `workout.label` as the literal
  string `"None"` beside real nulls, a session whose four samples and `mood` are all
  null, a tag with `end_time: null`, and heartrate rows with no `timestamp_unix` and a
  `producer_timestamp: null`.
- **`sleep/0-0.json`:** a `long_sleep` with the sandbox's full key set and nulls,
  beginning at 22:41 the evening before its Oura `day`; a `late_nap` with only the
  required keys; one `rest` and one `deleted` document, which aren't nights.
- **`workout/0-0.json`:** a walk with distance and calories and a null label, a
  `strength_training` with no `distance` key, null calories and the `"None"` label,
  and a run with a label the owner wrote.
- **`session/0-0.json`:** a meditation with samples, a breathing session with every
  sample null, a `nap` and a `body_status` session.
- **`enhanced_tag/0-0.json`:** a coded tag with no end, a `custom` tag with a name and
  an end, and a text-only tag with a null `tag_type_code`.
- **`heartrate/0-0.json`:** two rows in the sandbox's shape and one in the documented
  shape with `timestamp_unix`.
- **The eight daily endpoints:** one document each, in the sandbox's shapes, with a
  `daily_stress.day_summary` and a `daily_resilience.level` that judge the day, so a
  test can show they stay in `fields`.
- **`blood_glucose/0-0.json`:** a partner-gated endpoint absent from the published API,
  in the shape of `../oura_synthetic/blood_glucose.json`. It is counted and never
  stored.
