# Provenance Pattern

How cogitate agents communicate the basis and reliability of their claims. This pattern ensures briefings and reports distinguish between well-sourced facts and inferences.

Canonical implementation: `solstone/talent/morning_briefing.md` emitting `chronicle/<day>/talents/morning_briefing.json`.

## Four Mechanisms

### 1. Coverage Statement

A structured preamble summarizing what data sources were consulted and what gaps exist. Appears at the top of the output so the reader knows the briefing's evidence base before reading any claims.

**What it includes:**
- Source counts (segments, calendar events, entities consulted, newsletters, followups)
- Gaps — sources that returned zero results or errored
- YAML frontmatter with machine-readable source counts

**Example:**
> Built from 12 transcript segments, 4 calendar events, 3 entity profiles, 2 facet newsletters, and 5 follow-ups. Gaps: entity intelligence unavailable for Sarah Chen; no facet newsletters today.

### 2. Source Attribution

Inline parenthetical links connecting claims to their originating data using `sol://` URIs. Allows readers to trace any claim back to the source.

**When to attribute:**
- High-consequence factual claims (commitments, decisions, deadlines)
- Entity context drawn from specific interactions
- Follow-up items and action items

**When attribution is unnecessary:**
- Calendar events (self-evident from the calendar source)
- Reading section links (inherently attributed)
- General summaries that synthesize multiple sources

**Example:**
- `Closed the auth migration PR ([work newsletter](sol://facets/work/news/20260326))`
- `Last discussed launch timeline (from your [March standup](sol://20260313/archon/091500_300))`

### 3. Confidence-Graded Language

Claims read differently based on evidence strength. The agent expresses confidence through word choice — not numeric scores or metadata.

**Three tiers:**

| Level | Evidence | Language | Example |
|-------|----------|----------|---------|
| **High** | Multiple corroborating sources, explicit statement, or upstream confidence ≥ 0.85 | Assertive — no hedging | "Shipped the entity pipeline refactor." |
| **Medium** | Single clear source, or upstream confidence 0.50–0.84 | Attributed — direct but sourced | "Discussed pipeline issues in your March standup." |
| **Low** | Inference, ambiguous mention, single passing reference, or upstream confidence < 0.50 | Hedged — "possible," "may," "appears to" | "Possible progress on the auth migration." |

**Bidirectional rule:** Strong evidence must NOT get hedging language. Weak evidence must NOT get assertive language. Both directions must be enforced.

**Upstream confidence scores:** Some upstream sources emit a `Confidence: 0.0–1.0` field per item or row. When consuming this output, use the score to inform language grading. The briefing expresses confidence through language, not by forwarding the numeric score.

### 4. Tool Error Guard

When a data-gathering tool returns an error, the agent must handle it safely:

1. Record the error as a gap — never treat error message text as data
2. Note the gap in the coverage preamble
3. Continue the briefing using whatever data succeeded
4. For entity intelligence failures: still mention the person using available data (calendar, attendee list) and append "(entity context unavailable)"
5. Never omit a person solely because their entity intelligence failed
6. Never fabricate context to fill a gap

## Adoption Guide

To add provenance to a new cogitate agent:

1. **Add a pre-pass audit phase** after data gathering. Count sources, identify gaps, catalog tool errors. This becomes the basis for your coverage statement.

2. **Add source attribution rules** to your synthesis instructions. Define which claims need `sol://` links and which are self-evident. Use the URI construction rules from the morning briefing as a template.

3. **Add confidence-graded language rules** inline within each output section's instructions. Tailor the high/medium/low examples to that section's data sources — entity context grades differently than action items. State both directions (assert strong evidence, hedge weak evidence).

4. **Add a coverage preamble** to your output format. Include YAML frontmatter with source counts and a human-readable summary sentence with gaps.

5. **Add tool error guard instructions** to your data-gathering phase. Define how to handle errors for each tool the agent calls, especially entity lookups and journal searches.

## What This Pattern Does NOT Cover

- **Visual differentiation** — UI-level styling (bold, color, icons) to distinguish confidence levels. That belongs in the convey layer.
- **Automated confidence scoring** — Computing numeric confidence as metadata fields. Confidence is expressed through language only.
- **Retroactive provenance** — Applying provenance to historical briefings already generated. The pattern applies going forward.
- **Cross-agent provenance chains** — Tracing a claim through multiple agent hops (e.g., transcript → followups → briefing). Each agent applies provenance to its own output independently.
