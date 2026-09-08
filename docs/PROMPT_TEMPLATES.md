# Prompt templates

Talent instructions are Markdown files with optional JSON frontmatter. Rust loads their definitions, composes shared prompt fragments, and supplies request context before execution. There is no Python `load_prompt()` API in the runtime.

## Definitions and composition

Shipped definitions live in `core/payload/solstone/talent/`. App talents are discovered under each app's `talent/` directory. The definition's explicit `type` selects `generate` or `cogitate`; the presence of `tools` alone does not select an execution type.

Frontmatter uses an opening `{` and closing `}` on their own lines. YAML frontmatter is not supported. For example:

```markdown
{
  "title": "Activity synthesis",
  "type": "generate",
  "schedule": "segment"
}

$segment_preamble

Describe the activity supported by the supplied material.
```

[`solstone-core-talent-config`](../core/crates/solstone-core-talent-config/src/lib.rs) owns `read_frontmatter`, discovery, overrides and validation. Its `TalentConfig` carries the key, file, metadata and body.

[`compose_talent`](../core/crates/solstone-core-talent-cli/src/compose.rs) validates execution settings and returns a configuration map containing `user_instruction`. It also resolves schemas and input-source settings. `compose_talent_instruction` renders a definition's body with a journal root, template directory, optional focused facet and `BTreeMap<String, String>` context. The runtime's [preparation path](../core/crates/solstone-core-talent-runtime/src/prepare.rs) recomposes the instruction after merging request context.

## Substitution rules

The [template implementation](../core/crates/solstone-core-talent-cli/src/templates.rs) accepts `$name` and `${name}`, preserves unknown or malformed placeholders, and turns `$$` into a literal dollar sign. Identifiers use ASCII letters, digits and underscores, starting with a letter or underscore. Substitution does not recursively expand inserted values.

Composition proceeds in this order:

1. Flatten supported fields from `identity` in `config/journal.json`. Nested fields such as `pronouns.subject` become `pronouns_subject`. Missing, blank or path-shaped owner names are not inserted.
2. Add a default `$now` formatted in UTC. Caller context can override it and identity fields.
3. Add trimmed `identity/*.md` contents as `$identity_<stem>` when that key is not already supplied.
4. Load `.md` fragments from the template directory and render each against that same base variable map. Insert the rendered fragments by filename stem, then substitute the talent body. A fragment name takes precedence over a colliding base key in this final body pass.

Fragments can use identity and caller context. They do not recursively render one another. Avoid name collisions. A fragment-loading error leaves the body unsubstituted; an unreadable or invalid journal configuration propagates an error.

Identity and caller context also receive capitalized aliases: the first character is uppercased and the remainder lowercased, for both the key and value. For example, `$day_YYYYMMDD` has an alias `$Day_yyyymmdd`. Use the original variable when exact capitalization matters.

Shipped fragments include `daily_preamble.md`, `segment_preamble.md` and `activity_preamble.md` in [`core/payload/solstone/think/templates/`](../core/payload/solstone/think/templates/). Adding a file there makes its stem available during composition.

## Request context

Variables depend on the request and talent. They are not all globally available. The [request-context builder](../core/crates/solstone-core-talent-runtime/src/prompt_context.rs) supplies:

- Day: `$day` and `$day_YYYYMMDD`.
- Segment or span: `$segment` where applicable, `$segment_start` and `$segment_end`.
- Source: `$stream`, `$content_description` and `$import_guidance`.
- Facet: `$facet` and `$activity_md_dir`; the composer supplies `$facets` for discovery or focused-facet guidance.
- Activity: `$activity_id`, `$activity_type`, `$activity_description`, `$activity_level`, `$activity_entities`, `$activity_segments`, `$activity_duration` and `$activity_context` when their required inputs exist.
- Weekly bounds: `$week_end_YYYYMMDD` is six calendar days after the request day; `$lookback_start_YYYYMMDD` is six days before it. The talent determines whether its request day is a start or an as-of anchor.

Talent-specific preparation can add variables later. [`apply_template_vars`](../core/crates/solstone-core-talent-runtime/src/lib.rs) substitutes those values in `user_instruction`, `transcript` and `prompt`. Inspect the owning talent's preparation code before depending on a variable. Do not treat the default `$now` as the journal's local calendar date.

## Validation references

The existing tests in `templates.rs` cover escaping, unresolved placeholders, identity fields, fragment composition and invalid configuration. `compose.rs` covers execution configuration and facet/schema composition. Runtime `prompt_context.rs` tests cover dates, spans, streams, activity context and weekly boundaries.

For identity settings, see the [journal configuration reference](../core/payload/solstone/talent/journal/references/config.md). For execution settings and model options, use [THINK.md](THINK.md) and the talent configuration validators.
