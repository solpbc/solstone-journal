{
  "type": "generate",

  "title": "entity observer",
  "description": "reconciles proposed entity observations against existing observations",
  "color": "#004d40",
  "schedule": "daily",
  "priority": 58,
  "multi_facet": true,
  "group": "Entities",
  "output": "json",
  "schema": "entity_observer.schema.json",
  "thinking_budget": 2048,
  "hook": {"pre": "entities:entity_observer", "post": "entities:entity_observer"},
  "load": {"transcripts": false, "percepts": false, "talents": false}
}

## Core Mission

Reconcile proposed entity observations against current observations. For each suggested fact, decide whether to replace an existing observation, add a new observation, or skip.

## Pre-computed Context

Below you'll find the pre-computed context for this reconciliation run, including:
- Active entities with candidate suggestions for today
- Identity fields for each entity
- Suggested durable facts proposed for today (`S1..`)
- Recent live observations for each entity with `#id` and observation date

$observer_context

## Reconciliation Operations

For each suggestion, emit exactly one decision:

- `replace`: Use only when an existing observation was wrong or imprecise and the suggestion corrects it. Never for a fact that has since changed in the world — a changed fact is an `add`, and the older observation stays as the record of how things were. The target row is rewritten in place and its previous state is pushed to history. Provide `target_id` and the exact `target_quote` (up to 300 characters) matching the observation being replaced.
- `add`: Use when the suggestion is a durable fact not previously known, or a fact that changed in the world (a relationship that evolved, a role that moved). Older observations remain in place. The store automatically rejects exact duplicate content (across live and retired rows).
- `skip`: Use when an existing observation already covers the suggested fact or the suggestion is not needed.

## Rules

- Use the `entity_id` from context.
- Include every field on each decision; set non-applicable fields to `null`.
- For `replace`, `target_id` must match one of the `#id`s shown in the entity's current observations window, and `target_quote` must match text in that observation.
- For `add`, `content` is the durable fact text (up to 600 characters).
- For `skip`, set `content`, `target_id`, and `target_quote` to `null`.
- `relation` is `null` unless asserting a relationship. `target_name` is the other entity's NAME, never an ID. `kind` must be one of `works-with`, `works-at`, `reports-to`, `family-of`, `knows`, `uses`, `created`, `other`. `note` explains the relationship.
- `reasoning` is one short clause explaining the decision.
