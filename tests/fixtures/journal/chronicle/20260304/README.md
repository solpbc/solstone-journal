# Canonical Transcripts Reference Day

This day is the canonical fully analyzed reference day for transcripts dashboard visual review.

Every segment here is fully analyzed: screen has analysis lines, audio has real statements, and monitor diffs have real content.

Never add unanalyzed, smoke, stub, or `cost: 0.0` segments here. Never change the existing segment IDs, order, or count. The segment set is exactly three: `090000_300`, `140000_300`, `180000_300`.

The invariant is mechanically enforced by `tests/test_reference_day_fixture.py`. Treat that test as the source of truth for what fully analyzed means.

Known-good consumers depend on this day staying stable:

- transcripts day-view at `/app/transcripts/20260304`
- `solstone/apps/transcripts/tests/test_segment_routes.py`, which pins `FIXTURE_DAY`
- `tests/test_segment.py`, which validates segment chains
- activity-state-machine tests
- `tests/verify_browser.py`, which navigates this day
