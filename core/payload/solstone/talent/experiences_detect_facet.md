{
  "type": "generate",

  "title": "experiences detect (facet)",
  "description": "proposes focused lived experiences within one facet for a single day",
  "color": "#4f46e5",
  "schedule": "daily",
  "priority": 60,
  "multi_facet": true,
  "output": "json",
  "schema": "experiences_detect_facet.schema.json",
  "hook": {"pre": "experiences_detect_facet", "post": "experiences_detect_facet"},
  "load": {"transcripts": false, "percepts": false, "talents": false},
  "temperature": 0.2,
  "max_output_tokens": 2048
}

# Per-Facet Experience Detection

Inspect one facet's post-filtered activities for a single day (plus previous evening `>= 20:00` only when `00:00-03:00` rows exist) and propose focused, genuine lived experiences in this facet.

## Promote

1. **Substantive interpersonal, social, or world-facing episodes:**
   - Real conversations, 1:1 discussions, meals, lunches, dinners, family gatherings, hangouts, video calls, board or partner meetings, coaching sessions, rehearsals, live performances the owner takes part in, outings, walks, errands, or shop visits.
   - Can be one substantial standalone activity (for example, a 15-65 minute conversation, meal, or meeting) or a tight multi-activity cluster spanning channels (audio, calls, notes, messages, browser, photos, screenshots).
2. **Clear solo creative, engineering, or writing milestones (high bar):**
   - Solo work only when it forms a concrete milestone, completed submission, or multi-step deliverable arc (for example, drafting, verifying, and submitting a talk proposal; drafting a major chapter or scene across notes and drafts; reviewing a full pitch deck; completing a dedicated migration push).

## Skip (Do Not Promote and Do Not Absorb Into Neighboring Experiences)

- Passive television, movies, sports broadcasts, podcasts, overheard street performances, or silently sitting through a show where fictional or stage characters speak without active participation by the owner.
- Routine solo computer, terminal, ledger, copying, or folder-review blocks without a clear deliverable: ordinary 45-90 minute single-focus coding, terminal checks, inbox triage, or administrative upkeep.
- Trivial 1-3 minute micro-activities and sensitive administrative fragments: passive feed scrolling, passing remarks with bystanders, quick banking/medical/password/login checks, single-line ledger entries, standalone ambient audio, or routine single-turn messages.

## Tight Boundaries, Photos, and Scene Splitting

- Name one specific `topic_anchor` (3-6 words) before selecting `activity_handles`, and include only handles that directly belong to that exact event or deliverable (typically 1-6 handles; never sweep 10+ unrelated terminal, desk, or household rows into an all-morning or all-afternoon blob).
- Keep sequential and overlapping activities belonging to one continuous outing, gathering, meeting, performance, or deliverable together (bridging 10-120 minute lulls), skipping unrelated interleaved or overlapping distractions.
- Attach co-present `[photo/media]` rows (including `[no_story]` photos) taken during or within ~30 minutes of a lived outing, meal, walk, rehearsal, or event into that experience's `activity_handles`. Never emit a lone photo as its own experience.
- Split into separate experiences when a `>= 90` minute break pairs with a clear shift in people, place, or mode (for example, a morning rehearsal versus an afternoon performance, or a lunch with friends versus a solo afternoon errand).
- Include `[PREV-EVE]` handles only when a `00:00-03:00` handle directly continues that late-evening episode; include both the `[PREV-EVE]` and `00:00-03:00` handles together, and set `extends_experience_id` when continuing a listed open late-evening experience.

## Facet Activity Digest

$facet_digest

## Output

Return only a JSON object matching the schema. Do not wrap in a markdown fence.
