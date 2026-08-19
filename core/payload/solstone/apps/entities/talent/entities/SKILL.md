---
name: entities
description: >
  Tracked entities — people, companies, projects, tools — within facets.
  Detect, attach, move, merge/undo, resolve ambiguities, restore versions, update,
  alias, search, network, relationship history, entity history, overview.
  TRIGGER: entity, person, company, relationship, who is, contact, solstone call
  entities detect/attach/merge/search/network/history/overview.
---

# Entities CLI Skill

Maintain facet-scoped entity memory. Invoke via Bash: `solstone call entities <command> [args...]`.

**Environment defaults**: When `SOL_FACET` is set, all commands use it automatically. Same for `SOL_DAY` where DAY is accepted.

Common pattern:

```bash
solstone call entities <command> [args...]
```

## Entity Lifecycle

- **Detected**: day-scoped, ephemeral observations captured for a specific day.
- **Attached**: persistent entities tracked long-term in a facet.
- **Blocked**: entities the owner has blocked; do not detect, attach, or reuse.
- **Detached**: entities the owner removed; do not re-attach automatically.

Use `detect` for day-specific sightings and `attach` for long-term tracking.

## list

```bash
solstone call entities list [FACET] [-d DAY]
```

List entities for a facet.

- `FACET`: facet name (default: `SOL_FACET` env).
- `-d, --day`: optional day (`YYYYMMDD`).

Behavior notes:

- Without `--day`: lists attached (permanent) entities.
- With `--day`: lists detected entities for that day.

Examples:

```bash
solstone call entities list work
solstone call entities list work -d 20260115
```

## detect

```bash
solstone call entities detect TYPE ENTITY DESCRIPTION [-f FACET] [-d DAY]
```

Record a detected entity for a day.

- `TYPE`: entity type (alphanumeric + spaces, minimum 3 chars).
- `ENTITY`: entity id, full name, or alias.
- `DESCRIPTION`: day-scoped description.
- `-f, --facet`: facet name (default: `SOL_FACET` env).
- `-d, --day`: day in `YYYYMMDD` (default: `SOL_DAY` env).

Behavior notes:

- If `ENTITY` matches an attached entity, detection uses its canonical name.
- Blocked entities are rejected.
- Duplicate detections for the same day are rejected.

Example:

```bash
solstone call entities detect "Person" "Alicia Chen" "Led architecture review" -f work -d 20260115
```

## attach

```bash
solstone call entities attach TYPE ENTITY DESCRIPTION [-f FACET]
```

Attach an entity permanently to a facet.

- `TYPE`: required type.
- `ENTITY`: id, name, or alias reference.
- `DESCRIPTION`: persistent description.
- `-f, --facet`: facet name (default: `SOL_FACET` env).

Behavior notes:

- If already attached, command reports existing entity.
- Blocked entities are rejected.
- Previously detached entities are rejected.

Example:

```bash
solstone call entities attach "Company" "Acme Corp" "Primary platform vendor" -f work
```

## update

```bash
solstone call entities update ENTITY DESCRIPTION [-f FACET] [-d DAY]
```

Update entity description.

- `ENTITY`: entity id, name, or alias for attached entities; exact name for day-scoped detected updates.
- `DESCRIPTION`: new description.
- `-f, --facet`: facet name (default: `SOL_FACET` env).
- `-d, --day`: optional day (`YYYYMMDD`) to update a detected entity.

Behavior notes:

- Without `--day`: updates an attached entity.
- With `--day`: updates a detected entity for that day.

Examples:

```bash
solstone call entities update "acme_corp" "Primary vendor for identity services" -f work
solstone call entities update "Alicia Chen" "Discussed migration plan" -f work -d 20260115
```

## move

```bash
solstone call entities move ENTITY --from FACET --to FACET [--merge] [--consent]
```

Move an attached entity from one facet to another.

- `ENTITY`: entity name or partial match.
- `--from`: source facet.
- `--to`: destination facet.
- `--merge`: merge observations and relationship if an entity already exists in the destination.
- `--consent`: assert that explicit owner approval was obtained before the move (agent audit trail).

Behavior notes:

- Without `--merge`: fails if the entity already exists in the destination facet.
- With `--merge`: combines observations (deduplicated by content + observed_at) and preserves an existing relationship record; the source directory is removed afterwards.
- Both facets must exist.

Examples:

```bash
solstone call entities move "Alex Chen" --from personal --to work
solstone call entities move "Alex Chen" --from personal --to work --merge --consent
```

## aka

```bash
solstone call entities aka ENTITY AKA [-f FACET]
```

Add an alias to an attached entity.

- `ENTITY`: entity id, name, or alias reference.
- `AKA`: alias to add.
- `-f, --facet`: facet name (default: `SOL_FACET` env).

Behavior notes:

- Automatically skips aliases that equal the first word of the entity name.
- Automatically deduplicates existing aliases.
- Validates alias uniqueness across entities in the facet.

Example:

```bash
solstone call entities aka "Federal Aviation Administration" "FAA" -f work
```

## observations

```bash
solstone call entities observations ENTITY [-f FACET]
```

List durable observations for an attached entity.

- `ENTITY`: entity id, name, or alias.
- `-f, --facet`: facet name (default: `SOL_FACET` env).

Output is numbered for quick review.

Example:

```bash
solstone call entities observations "Alicia Chen" -f work
```

## observe

```bash
solstone call entities observe ENTITY CONTENT [-f FACET] [--source-day DAY]
```

Add a durable observation to an attached entity.

- `ENTITY`: entity id, name, or alias.
- `CONTENT`: observation text.
- `-f, --facet`: facet name (default: `SOL_FACET` env).
- `--source-day`: optional day (`YYYYMMDD`) when this was observed.

Behavior notes:

- Observation number is auto-calculated by the CLI.

### Observation Quality Guidance

Good observations (durable factoids):

- "Prefers async communication over meetings"
- "Works PST timezone, typically available after 10am"
- "Has deep expertise in distributed systems and Rust"
- "Reports to Sarah Chen on the platform team"

Bad observations (day-specific activity; use `detect` instead):

- "Discussed API migration today"
- "Sent contract for review"

Example:

```bash
solstone call entities observe "Alicia Chen" "Prefers design docs before implementation" -f work --source-day 20260115
```

## search

```bash
solstone call entities search [--query QUERY] [--type TYPE] [--facet FACET] [--since YYYYMMDD] [--limit N]
```

Search entities by text, type, facet, or detected activity since a day.

- `--query`: optional text query.
- `--type`: filter by entity type (e.g., `Person`, `Company`).
- `--facet`: filter by facet.
- `--since`: filter to entities detected on or after `YYYYMMDD`.
- `--limit`: maximum results.

Examples:

```bash
solstone call entities search --query "Chen"
solstone call entities search --type Person --facet work
solstone call entities search --since 20260115
```

## network

```bash
solstone call entities network ENTITY [--kinds KIND] [--facet FACET] [--day-from YYYYMMDD] [--day-to YYYYMMDD] [--limit N] [--evidence-limit N] [--include-principal] [--json]
```

Show one-hop recorded connections for a journal entity.

- `ENTITY`: entity id, name, or alias. Output uses the journal entity id slug.
- `--kinds`: optional edge kind filter; repeat the flag or comma-separate values.
- `--facet`: optional facet filter and resolution scope.
- `--day-from`, `--day-to`: optional evidence day bounds.
- `--limit`: maximum neighbors to show.
- `--evidence-limit`: evidence rows per neighbor.
- `--include-principal`: include the owner/principal entity when it would otherwise be hidden.
- `--json`: return the raw route payload.

Behavior notes:

- Empty output means the edge table exists but this entity has no recorded connections under the filters.
- Ranking counts only evidence dated up to today; each item carries an `evidence_class` of attendance, semantic, or mixed.
- If the edge index has not been built, the command tells the owner to run `journal indexer --rescan`.

Examples:

```bash
solstone call entities network "Alicia Chen" --facet work
solstone call entities network romeo_montague --limit 10 --evidence-limit 2
```

## history

```bash
solstone call entities history ENTITY [PEER] [--kinds KIND] [--facet FACET] [--day-from YYYYMMDD] [--day-to YYYYMMDD] [--limit N] [--offset N] [--json]
```

Show newest-first evidence rows for one entity pair.

- `ENTITY`: entity id, name, or alias.
- `PEER`: optional peer entity. If omitted, defaults to the principal entity.
- `--kinds`: optional edge kind filter; repeat the flag or comma-separate values.
- `--facet`: optional facet filter and resolution scope.
- `--day-from`, `--day-to`: optional evidence day bounds.
- `--limit`: maximum evidence rows.
- `--offset`: pagination offset.
- `--json`: return the raw route payload.

Behavior notes:

- Evidence rows include day, edge kind, available label/anchor, source, and path.
- History is the full honest pair record, including future-dated evidence rows.
- If no principal entity exists and `PEER` is omitted, the command fails and asks for `PEER`.

Examples:

```bash
solstone call entities history "Alicia Chen"
solstone call entities history "Alicia Chen" "Sam Rivera" --day-from 20260601
```

## overview

```bash
solstone call entities overview [--kinds KIND] [--facet FACET] [--day-from YYYYMMDD] [--day-to YYYYMMDD] [--limit N] [--json]
```

Show the global recorded-connections overview.

- `--kinds`: optional edge kind filter; repeat the flag or comma-separate values.
- `--facet`: optional facet filter.
- `--day-from`, `--day-to`: optional evidence day bounds.
- `--limit`: maximum connected entities to show.
- `--json`: return the raw route payload.

Behavior notes:

- Use this before `network` when you need to know which entities are most connected.
- Ranking counts only evidence dated up to today; each item carries an `evidence_class` of attendance, semantic, or mixed.
- The header reports total entities and whether the displayed list is truncated.

Example:

```bash
solstone call entities overview --facet work --limit 20
```

## merge

```bash
solstone call entities merge SOURCE_SLUG TARGET_SLUG [--commit/--no-commit] [--keep-source-as-aka/--no-keep-source-as-aka]
```

Plan or execute a merge of two journal entities (e.g., collapse duplicate records after review).

- `SOURCE_SLUG`: entity slug to merge from (removed on `--commit`).
- `TARGET_SLUG`: entity slug to merge into (canonical).
- `--commit/--no-commit`: default `--no-commit`. Returns a JSON plan with no mutations. Pass `--commit` to persist.
- `--keep-source-as-aka/--no-keep-source-as-aka`: default `--keep-source-as-aka`. Preserve the source display name as an alias on the target.

Behavior notes:

- Observations and aliases from source are merged into target with deduplication.
- If any other entity references `SOURCE_SLUG` in its alias list, the merge is rejected and the offenders are listed.
- If both entities have principals assigned, the merge is rejected.

Examples:

```bash
solstone call entities merge raelyn-brooks raylyn-brooks
solstone call entities merge raelyn-brooks raylyn-brooks --commit
```

## undo-merge

```bash
solstone call entities undo-merge MERGE_ID --yes [--json]
```

Deterministically undo one recorded merge. Use the `merge_id` returned by a
successful merge or accepted merge suggestion. This restores the recorded
source entity and its owned references without rolling back unrelated later
target changes. `--yes` is required.

## ambiguities and resolve-ambiguity

```bash
solstone call entities ambiguities [--status open|resolved] [--json]
solstone call entities resolve-ambiguity AMBIGUITY_ID ENTITY_ID --yes [--json]
```

List persisted low-confidence entity questions and choose an existing scoped
entity. The list includes candidates and origin lanes. A recorded choice sticks
for later mutation runs. Never resolve to a blocked, missing, or out-of-scope
entity. `resolve-ambiguity` requires `--yes`.

## entity-history and restore-version

```bash
solstone call entities entity-history ENTITY_ID [--json]
solstone call entities restore-version ENTITY_ID VERSION_ID --yes [--json]
```

`entity-history` shows durable identity versions (create, update, restore,
merge, merge undo). This is distinct from `history`, which shows relationship
evidence between two entities. Restore only ordinary identity versions;
merge-bearing history must use `undo-merge`. `restore-version` requires `--yes`.

## Gotchas

- **`merge` previews by default.** Default is `--no-commit`: it emits a JSON plan without mutating anything. Pass `--commit` when you actually want the merge to happen.
- **`history` and `entity-history` are different.** `history` reads relationship evidence; `entity-history` reads restorable identity versions.
- **Undo and restore require explicit confirmation.** Pass `--yes`; the commands make no request without it.
- **`detect` requires TYPE ≥ 3 chars.** Shorter types are silently rejected.
- **`observe` is for durable traits, `detect` is for day-scoped sightings.** Mixing them skews future entity context.
