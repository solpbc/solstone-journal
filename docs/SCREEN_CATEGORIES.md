# Screen Description Categories

Category definitions for vision analysis of screencast frames live in
`core/crates/solstone-core-describe-categories/assets/categories/`.
`solstone-core-describe-categories` embeds them.

## Adding a New Category

Add a `.md` file in that crate's `assets/categories/` with JSON frontmatter
and an optional extraction prompt. Register it in the crate's `SOURCES` table.

### 1. `<category>.md` (required)

Defines the category with JSON frontmatter and optional extraction prompt:

```markdown
{
  "description": "One-line description for categorization prompt",
  "output": "markdown",
  "max_output_tokens": 4096
}

Optional extraction prompt content goes here...
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `description` | Yes | - | Single-line description used in the categorization prompt |
| `output` | No | `"markdown"` | Response format for extraction: `"json"` or `"markdown"` |
| `max_output_tokens` | No | `4096` | Maximum output tokens for category-specific extraction |

Model selection is handled via the providers configuration in `journal.json`. Each category uses the context pattern `observe.describe.<category>` for routing. See [config.md](../talent/journal/references/config.md) for details on configuring providers per context.

Categories with prompt content after the frontmatter are "extractable" - they can receive detailed content extraction after initial categorization. The prompt is sent to the model for analysis and should instruct the model to:
- Analyze the screenshot for this specific category
- Return content in the format specified by `output` (markdown or JSON)

### 2. `<category>.schema.json` (required for JSON output)

Defines the strict structured-output schema for categories with `"output": "json"`. The file is discovered by filename convention next to the markdown prompt (`<category>.schema.json`).

JSON category schemas must satisfy the following. ⚠ The checker that enforced them (`scripts/check_schema_bounds.py`, with a pytest companion) read the deleted Python tree and has been removed, so these are currently conventions rather than gated rules:
- Every array must have `maxItems`
- Every free-text string must have `maxLength`
- Every object must set `additionalProperties:false`
- Every object must list all properties in `required`
- Do not use `oneOf`; express nullability with type lists such as `["string", "null"]`

There is no per-category Python formatter. Default formatting applies:
markdown with a category header, JSON in a code block.

## How It Works

1. `solstone-core-describe-categories` is the source of category metadata
2. **Phase 1 (Categorization)**: All frames get initial category analysis (primary/secondary)
3. **Phase 2 (Selection)**: Which frames get detailed extraction (`describe.max_extractions`)
4. **Phase 3 (Extraction)**: Selected extractable categories get detailed content extraction
5. Results are stored in JSONL with `enhanced: true/false`
