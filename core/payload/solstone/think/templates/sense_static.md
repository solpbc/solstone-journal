$facets

$segment_preamble

# Segment Sense (Written, Document, Note & Static Image)

Read this historical or static segment and return ONE JSON object covering `density`, `content_type`, `activity_summary`, `facets`, `speculative_facet`, `entities`, and `topics`.

## Field Instructions

### density
- `active`: Meaningful human content, correspondence, note-taking, document reading, or focused activity (>5 lines, >30 words, or clear owner/correspondent action).
- `low_change`: Minimal content (<5 words) with no clear action.
- `idle`: Empty or system noise only.

### content_type
Pick the single best match from the schema enum (`email`, `messaging`, `ai_conversation`, `writing`, `reading`, `browsing`, `coding`, `design`, `planning`, `productivity`, `social`, `media`, `idle`, etc.).
- Written threads/emails -> `email` or `messaging`; AI chats (ChatGPT, Claude, Gemini) -> `ai_conversation`; markdown/vault notes -> `writing`; reference docs/PDFs/newsletters -> `reading`.

### activity_summary
- Start with a **past-tense verb**, with **no subject/name/pronoun before the verb** when the owner acted (e.g., `"Replied to Samira Patel about meeting at South Mesa trailhead at 7:30."`, `"Reviewed Dev Kapoor's pull request fixing raft lease renewal."`).
- Preserve who did what: if someone else acted or sent a message the owner received/read, name them accurately (`"Received Dev Kapoor's forwarded cloud storage quote for Q2 budget review."`).
- A document, forward, quote, receipt, or image establishes its contents; **never infer** that the owner paid an invoice, signed a contract, or executed a plan unless explicitly stated.
- When working with an AI assistant, say what the owner asked or decided, never attribute the assistant's generated explanation as the owner's own work.
- Write `"solstone"` in lowercase if referencing the product.

### facets
Classify into the owner's configured facets (`minItems: 1`).
- `facet`: MUST be one of the configured facet slugs listed above. Never invent a slug here.
- `activity`: 1-sentence past-tense summary for that facet (same verb-first rule as `activity_summary`).
- `level`: `"high"` (primary focus), `"medium"` (meaningfully involved), or `"low"` (loose fallback match when no configured facet truly fits). Check each facet's stated description carefully: if the segment belongs to a distinct hobby, former company, or life area outside the facet's stated scope (e.g., sailing vs trail running, pottery vs family), mark `level: "low"` and propose that area in `speculative_facet`.

### speculative_facet
- Emit `null` whenever at least one entry in `facets[]` is `"high"` or `"medium"`.
- ONLY when **every** entry in `facets[]` is `"low"` (none of the owner's current facets fits this historical life/work area), propose a concise Title Case folder name grounded in the content (e.g., `"Kestrel Robotics"`, `"Sailing"`, `"Ceramics Studio"`). Never use snake_case or kebab-case.

### entities
Extract named `Person`, `Company`, `Project`, and `Tool` entities grounded in the segment (`$preferred` is the journal owner; **NEVER list `$preferred` or `$owner_name` as an entity**).
- Prefer full natural names; when a given name or surname matches a known entity in `$facets` above, use that entity; otherwise skip ambiguous bare first names. Skip URLs, file paths, branch names, and raw ticket IDs. AI assistants (ChatGPT, Claude, Gemini) are `Tool`, never `Person`.
- `role`: `"mentioned"` (default for written/static items; `"attendee"` is only for live calls).
- `context`: 1 concrete phrase of what it did or how it was involved **in this segment** (not a generic bio).
- `level`: `"high"` (central), `"medium"` (meaningfully involved), or `"low"` (peripheral/passing).
- `facet` (**genuine involvement gate**):
  - Set `facet` to **the single active facet slug** (from `facets[]` above) **ONLY if this entity genuinely took part in what happened here or got directly worked on**, such as an active sender/recipient in a direct exchange with the owner, a collaborator or subject directly reviewed/worked on, a person/project/company directly depicted or diagrammed in an owner photo, or the core project/company/tool being worked on in the foreground.
  - Set `facet` to **`null`** for **all scenery and passive mentions**: passive `Cc:` bystanders on group emails, bulk/newsletter senders (`Substack`, `noreply`), authors/celebrities/companies merely referenced in a newsletter or article, unfocused browser tabs, bookmarks bars, Slack sidebar unread names, music players/OS menu-bar apps in screenshot chrome, or vendor sales desks in bare quotes/receipts.

### topics
Up to 10 specific search phrases someone would type later to find this moment again (projects, places, technical terms, events, amounts, problems, such as `"Shadow Canyon loop"`, `"IORING_REGISTER_BUFFERS"`, `"Whitehall warrant"`). Use the content's own words; do not list bare person names alone.

Return ONLY valid JSON matching the schema.
