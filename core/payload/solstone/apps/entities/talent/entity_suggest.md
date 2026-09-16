{
  "type": "generate",

  "title": "entity suggest",
  "description": "proposes candidate durable factoids about attached entities from journal content",
  "color": "#004d40",
  "schedule": "daily",
  "priority": 57,
  "multi_facet": true,
  "group": "Entities",
  "output": "json",
  "schema": "entity_suggest.schema.json",
  "thinking_budget": 2048,
  "hook": {"pre": "entities:entity_suggest", "post": "entities:entity_suggest"},
  "load": {"transcripts": false, "percepts": false, "talents": false}
}

## Core Mission

Propose candidate durable factoids about attached entities from recent journal content. Observations are persistent facts that help with future interactions - preferences, expertise, relationships, schedules, and biographical details. This is NOT about logging daily activity (that's entity detection), but capturing lasting knowledge.

## Pre-computed Context

Below you'll find the pre-computed context for this suggestion run, including:
- Active entities that appeared in today's content
- Identity fields for each entity: name, type, description, and aliases
- Fresh detection summaries and source evidence from today's segments

$suggest_context

## What Makes a Good Observation Suggestion

**The litmus test** — an observation must pass BOTH:
1. "Would this be true and useful 6 months from now, even without knowing when it was observed?"
2. "Would this help someone who's never interacted with this entity understand or work with them?"

If either answer is no, it's not an observation — it's activity, and belongs in detection.

**DO suggest** — durable factoids about WHO or WHAT the entity IS:
- Personality/style: "Advocates for Socratic questioning in mentorship"
- Preferences: "Prefers async communication over meetings"
- Expertise: "Has deep knowledge of distributed systems and Rust"
- Relationships: "Reports to Sarah Chen on the platform team"
- Schedule/patterns: "Works PST timezone, typically available after 10am"
- Biographical: "Based in Seattle, previously worked at Google"
- Working style: "Challenges speculative answers and pushes for validation before accepting changes"

**DON'T suggest** — these are NOT observations:
- Day-specific activity: "Discussed migration today", "Sent contract for review"
- Scheduled events: "OOO on Thursday Jan 22", "Surgery needs scheduling by next week"
- Version/point-in-time state: "Uses v2.1.50", "Currently fails under Bun" — these expire
- Usage logs: "Used X to refactor Y", "Acted as primary tool for Z" — activity, not identity
- News/announcements: "Reopened comment period in January" — events that happened
- Compound facts: "Did A; also B; and C" — if you can't say it in one focused sentence, split or pick the most durable one
- Anything with "currently", "as of", or "today" — these signal ephemeral state

## Rules

- For each selected entity, suggest at most three strictly-qualifying facts.
- Content must be at most 600 characters per suggestion.
- One fact per suggestion — no compound sentences.
- `relation` is `null` unless the fact asserts a relationship. `target_name` is the other entity's NAME, never an ID. `kind` must be one of `works-with`, `works-at`, `reports-to`, `family-of`, `knows`, `uses`, `created`, `other`. `note` explains the relationship.
- `reasoning` is one short clause explaining why this fact passes the durability litmus.
- If an entity has no qualifying durable facts in today's content, return an empty suggestions array for that entity.
