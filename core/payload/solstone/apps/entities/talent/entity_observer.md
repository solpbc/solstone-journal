{
  "type": "generate",

  "title": "Entity Observer",
  "description": "Extracts durable factoids about attached entities from journal content",
  "color": "#004d40",
  "schedule": "daily",
  "priority": 57,
  "multi_facet": true,
  "group": "Entities",
  "output": "json",
  "schema": "entity_observer.schema.json",
  "thinking_budget": 2048,
  "hook": {"pre": "entities:entity_observer", "post": "entities:entity_observer"},
  "load": {"transcripts": false, "percepts": false, "talents": false}
}

## Core Mission

Extract durable factoids about attached entities from recent journal content. Observations are persistent facts that help with future interactions - preferences, expertise, relationships, schedules, and biographical details. This is NOT about logging daily activity (that's entity detection), but capturing lasting knowledge.

## Pre-computed Context

Below you'll find the pre-computed context for this observation run, including:
- Active entities that appeared in today's content
- Identity fields for each entity: name, type, description, and aliases
- Full current observations, numbered from 0, which are the targets for update, drop, and keep operations
- Fresh source evidence: sense context, transcript excerpts, related journal evidence, and knowledge-graph chunks

$observer_context

## What Makes a Good Observation

**The litmus test** — an observation must pass BOTH:
1. "Would this be true and useful 6 months from now, even without knowing when it was observed?"
2. "Would this help someone who's never interacted with this entity understand or work with them?"

If either answer is no, it's not an observation — it's activity, and belongs in detection.

**DO capture** — durable factoids about WHO or WHAT the entity IS:
- Personality/style: "Advocates for Socratic questioning in mentorship"
- Preferences: "Prefers async communication over meetings"
- Expertise: "Has deep knowledge of distributed systems and Rust"
- Relationships: "Reports to Sarah Chen on the platform team"
- Schedule/patterns: "Works PST timezone, typically available after 10am"
- Biographical: "Based in Seattle, previously worked at Google"
- Working style: "Challenges speculative answers and pushes for validation before accepting changes"

**DON'T capture** — these are NOT observations, even when they feel factual:
- Day-specific activity: "Discussed migration today", "Sent contract for review"
- Scheduled events: "OOO on Thursday Jan 22", "Surgery needs scheduling by next week"
- Version/point-in-time state: "Uses v2.1.50", "Currently fails under Bun" — these expire
- Usage logs: "Used X to refactor Y", "Acted as primary tool for Z" — activity, not identity
- News/announcements: "Reopened comment period in January" — events that happened, not facts about the entity
- Compound facts: "Did A; also B; and C" — if you can't say it in one focused sentence, split or pick the most durable one
- Anything with "currently", "as of", or "today" — these signal ephemeral state

### Observation Strategy by Entity Type

Different entity types yield different kinds of durable knowledge:

- **People**: Personality, communication style, expertise areas, working patterns, relationships, decision-making tendencies, timezone/schedule. These are the richest entities. Prioritize WHO they are over WHAT they did.
- **Companies/Orgs**: Strategic position, culture, key business relationships, decision-making patterns, organizational structure. NOT news events or quarterly status.
- **Projects**: Architecture decisions, design principles, known constraints, key technical learnings. NOT commit logs or deployment activity.
- **Tools**: Capabilities, limitations, best-practice configurations. NOT "was used for X on Y" — that's a usage log, not a fact about the tool.

## Operations

Use operations to maintain the numbered current observations shown in context:

- `update`: revise an existing numbered observation when fresh source evidence improves, narrows, or corrects it.
- `drop`: remove an existing numbered observation when it is duplicated, stale, or fails the durability litmus.
- `add`: append a new durable fact that is not already covered by the current observations.
- `keep`: deliberately re-affirm an existing numbered observation against fresh confirming or contradicting evidence.

Rules:
- Use the `entity_id` from context.
- Include every field on each operation; set non-applicable fields (`target_index`, `content`, `target_quote`, `relation`) to `null`.
- Prefer `update` or `drop` over adding a near-duplicate observation.
- For `update` and `drop`, include a verbatim `target_quote` of at most 300 characters from the target observation. Use a short identifying excerpt; do not copy the whole observation when it exceeds that limit.
- At most one operation may target a given observation index for an entity.
- Use `add` only for facts that pass the durability litmus.
- One fact per observation — no compound sentences.
- `relation` is `null` unless the observation asserts a relationship. `target_name` is the other entity's NAME, never an id. `kind` must be one of `works-with`, `works-at`, `reports-to`, `family-of`, `knows`, `uses`, `created`, `other`. `note` explains the relationship and is required when `kind` is `"other"`.
- For `add`, `update`, and `drop`, `reasoning` is one short clause, well under the cap.
- For `keep`, set `reasoning` to `null`.
- Emit `keep` only when fresh evidence makes an explicit re-affirmation useful; otherwise omit the operation.
- An entity with no operations is valid and preferred when no changes or re-affirmations are needed.

## Output Format

Respond with a JSON object in this exact format:

```json
{
  "entities": [
    {
      "entity_id": "alice_johnson",
      "operations": [
        {
          "op": "update",
          "target_index": 0,
          "content": "The revised durable observation text",
          "target_quote": "short exact quote from the old observation",
          "reasoning": "Fresh evidence narrows it",
          "relation": null
        },
        {
          "op": "add",
          "target_index": null,
          "content": "A new durable observation text",
          "target_quote": null,
          "reasoning": "Durable uncaptured expertise",
          "relation": {
            "kind": "works-with",
            "target_name": "Bob Lee",
            "note": ""
          }
        },
        {
          "op": "drop",
          "target_index": 2,
          "content": null,
          "target_quote": "short exact quote from the old observation",
          "reasoning": "Stale duplicate",
          "relation": null
        },
        {
          "op": "keep",
          "target_index": 3,
          "content": null,
          "target_quote": null,
          "reasoning": null,
          "relation": null
        }
      ]
    }
  ],
  "summary": "Updated X entities with Y operations."
}
```
