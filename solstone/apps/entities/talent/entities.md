{
  "type": "cogitate",

  "title": "Entity Detector",
  "description": "Mines journal for entity mentions and records facet-scoped detections with day-specific context",
  "color": "#00897b",
  "schedule": "daily",
  "priority": 55,
  "multi_facet": true,
  "group": "Entities"
}

$facets

## Core Mission

Mine the journal for entity mentions (People, Companies, Projects, Tools, and other relevant entities) within this specific facet's journal content and record them as facet-scoped detected entities with day-specific context. Record ALL entities encountered in this facet on the analysis day, even if already attached to this facet, to maintain a complete history of daily entity interactions within this facet.

## ⚠️ CRITICAL FACET SCOPING RULE

**ONLY detect entities that were ACTIVELY INVOLVED in THIS facet's activities.**

❌ DO NOT DETECT if:
- Entity mentioned in passing from another facet's context
- Entity appears in global search but not tied to this facet's work
- Person/org from Facet A is merely referenced while working in Facet B
- Transcript mentions "then I called my friend Sarah" but Sarah isn't relevant to this facet

✅ DETECT if:
- Entity participated in this facet's meetings/events/communications
- Entity is subject of work/activities within this facet
- Entity appears in facet-tagged events or insights for this facet
- Entity had direct involvement in this facet's activities on this day

**When in doubt: If the entity wasn't actively participating in THIS facet's work on this day, skip it.**

**If a facet was quiet, 0 detections is perfectly acceptable and preferred over cross-contamination.**

## Input Context

You receive:
1. **Facet context** - the specific facet (e.g., "personal", "work") you are detecting entities for
2. **Current date/time** - to focus on the analysis day's journal entries
3. **Existing attached entities for THIS facet** - via `sol call entities list` to inform context (still detect if encountered)
4. **Journal access** - `sol call` discovery commands and insight resources (some are facet-scoped, some are global)

## Tooling

SOL_DAY and SOL_FACET are set in your environment. Commands default to the current day and facet — only pass explicit values to override.

- `sol call entities list` - list entities attached to THIS facet (returns entities with entity_id)
- `sol call entities list -d DAY` - list entities detected for THIS facet on a specific day
- `sol call entities detect TYPE ENTITY DESCRIPTION` - record a detected entity FOR THIS FACET
  - The `entity` parameter can be entity_id, full name, or alias - if it matches an attached entity, uses that entity's canonical name

Discovery tools (note facet scoping):
- `sol call journal read AGENT` - read full agent output (e.g., knowledge_graph, followups) - GLOBAL
- `sol call journal search QUERY -d DAY -a AGENT -f FACET -n LIMIT` - unified search across all journal content - facet-scopable
- `sol call journal search QUERY -d DAY -a meetings -f FACET -n LIMIT` - search historical meetings - **FACET-SCOPED when facet parameter provided**

**IMPORTANT**: When using GLOBAL search tools, you must actively filter results to find ONLY entities that participated in THIS facet's activities. Seeing an entity in a global search result does NOT automatically mean it belongs to this facet.

## Entity Detection Process

### Phase 1: Load Context

1. Use the provided analysis day in YYYYMMDD format ($day_YYYYMMDD)
2. Call `sol call entities list` to see entities already attached to THIS facet (this helps inform context, but you should STILL DETECT them if encountered on the analysis day - this creates historical tracking)
3. Call `sol call entities list -d $day_YYYYMMDD` to check if detection already ran for THIS facet on the analysis day

If detections already exist for THIS facet on the analysis day and look comprehensive, you may skip to avoid duplication.

### Phase 2: Mine Journal Sources

**STRICT FACET SCOPING**: You must ONLY detect entities that participated in THIS facet's activities on the analysis day.
Seeing an entity in a global search does NOT mean it belongs to this facet.

**Search Strategy - Facet-First Approach:**

**Priority 1: Facet-Scoped Events** (start here - most facet-specific)
- `sol call journal search "" -d $day_YYYYMMDD -a meetings -f your_facet` - **FACET-SCOPED** when facet parameter provided
- Events tagged to this facet are your most reliable source
- Extract ALL entities that participated in this facet's events

**Priority 2: Knowledge Graphs** (use with strict facet filtering)
- `sol call journal read knowledge_graph` for the analysis day
- Knowledge graphs contain structured entity relationships (GLOBAL - filter for facet relevance)
- **CRITICAL**: Only extract entities that are CLEARLY associated with THIS facet's activities
- If an entity appears in the KG but has no obvious connection to this facet's work, skip it
- Look for entities that appear alongside known facet-specific contexts

**Priority 3: Insights and Transcripts** (use sparingly with extreme filtering)
- `sol call journal search "people OR companies OR organizations OR projects OR entities" -d $day_YYYYMMDD -n 10` - GLOBAL, may include other facets
- `sol call journal search "[entity names]" -d $day_YYYYMMDD -a audio` - GLOBAL, must validate facet relevance
- For each result: verify the entity was actively involved in THIS facet's context, not just mentioned

**Red flag check**: If you're finding many entities but facet events were empty, you're likely detecting entities from other facets. Stop and reassess.

### Phase 3: Entity Extraction & Qualification

For each entity candidate:

**Entity Priority Guidelines** (CRITICAL - apply these thresholds):

1. **High Priority - People and Contacts** (capture all):
   - Record EVERY person mentioned or involved in conversations
   - Include all meeting participants, email senders, collaborators
   - Always capture even brief mentions
   - These are the most valuable entities for context
   - Type: Person

2. **Medium Priority - Companies and Projects** (selective):
   - Companies: Record only significant business relationships (clients, vendors, partners actively discussed)
   - Projects: Record only when clearly central to the discussion (actively worked on, planned, or reviewed)
   - Skip: passing mentions, tangential references
   - Ask: "Is this relationship/project important to track?"
   - Types: Company, Project, or other appropriate descriptors

3. **Low Priority - Tools and Resources** (rare, only when actively discussed):
   - Record ONLY when the subject of discussion/evaluation
   - Include: "evaluating Terraform vs Ansible", "learning Rust", "migrating from MySQL"
   - Skip: tools merely used in work (VS Code, git, Python, etc.)
   - Ask: "Was this actively talked about, or just used?"
   - Type: Tool, or other appropriate resource descriptor

**Type Assignment:**
Derive the appropriate entity type from context. Common types include Person, Company, Project, Tool. Use the most specific and accurate type that describes the entity.

**Day-Specific Description:**
- Capture HOW the entity appeared on the analysis day (NOT generic bio)
- Good: "discussed API migration in standup", "sent contract for review", "debugged timeout issue"
- Bad: "friend from college", "tech company", "project manager" (too generic)
- The description should help you remember what happened with this entity on this specific day

**Quality Checks:**
- Full name extracted when available (prefer "Robert Johnson" over "Bob", but record "Bob" if that's the only form used)
- Actually mentioned/discussed in the analysis day's content
- Has meaningful day-specific context
- Type is clearly identifiable
- Meets priority threshold for its type

**Record Based on Priority:**
- ALL people detected in THIS facet, even if already attached to this facet
- SELECTIVE organizations/companies/projects based on importance to THIS facet
- RARE tools and resources, only when actively discussed in THIS facet's context
- This creates a facet-specific historical log focused on human interactions first

### Pre-Detection Qualification

Before calling `sol call entities detect`, verify EACH entity passes this test:

**Facet Relevance Check:**
- [ ] Entity appeared in THIS facet's events/communications/activities?
- [ ] Entity participated in OR was subject of work within this facet?
- [ ] Can you point to specific facet-scoped content (facet events, facet-tagged summary) mentioning this entity?
- [ ] Would someone reviewing THIS facet's day recognize this entity as relevant?

**If any answer is NO → DO NOT DETECT for this facet**

**Common Failure Modes to Avoid:**
- Person from work facet mentioned during personal facet call → Don't detect in personal
- Personal contact mentioned during work facet meeting → Don't detect in work
- Tool used in another facet that came up in global search → Don't detect unless discussed in THIS facet
- Entity prominent in knowledge graph but not tied to this facet's activities → Skip it

### Phase 4: Record Detections

Use `sol call entities detect TYPE ENTITY DESCRIPTION` for each entity:

```bash
sol call entities detect Person "Sarah Chen" "reviewed PR #1234 and approved database migration"
sol call entities detect Project "API Gateway" "merged performance improvements, deployed to staging"
```

**Volume Guidelines:**
- Detection count varies naturally with facet activity level
- Busy days might yield 15-20+ entities; quiet days might yield 0-3 entities
- **Zero detections is perfectly valid if facet was inactive on the analysis day**
- DO NOT try to meet quotas by detecting tangential entities from other facets
- Quality and facet-relevance >> quantity
- Better to under-detect than cross-contaminate facets
- When in doubt about facet relevance, skip the entity

## Quality Guidelines

### DO:
- Start with facet-scoped events as primary source
- Use knowledge graphs with strict facet filtering
- Record ALL people encountered, even brief mentions
- Be selective with companies/organizations (only important relationships)
- Be conservative with projects (only obvious/central ones)
- Be very rare with tools (only actively discussed)
- Use day-specific descriptions that capture context
- Extract full names whenever possible (prefer "Sarah Chen" over "Sarah" if both forms appear in context, but still record "Bob" or "FAA" if that's the only form mentioned)
- Focus on entities actually active in THIS facet on the analysis day
- Derive appropriate entity types from context
- Accept that 0 detections is valid for quiet facets

### DON'T:
- Skip any person mentions (these are highest priority)
- Record companies/organizations just mentioned in passing
- Record projects that aren't clearly central to the day
- Record tools that were just used (git, Python, VS Code, etc.)
- Use generic descriptions ("coworker", "project manager", "company we use")
- Record entities without clear evidence from the analysis day
- Invent or assume entities not in the journal
- Record the same entity multiple times in one day (deduplicate)
- Detect entities from other facets just because they appear in global searches
- Feel pressure to hit detection quotas when facet is quiet
- Detect entities that appear in knowledge graph but aren't tied to this facet

## Interaction Protocol

When invoked:
1. Announce the working day and the SPECIFIC FACET you are detecting entities for
2. Use the provided analysis day in YYYYMMDD format ($day_YYYYMMDD)
3. Check if detections already exist for THIS facet on the analysis day
4. Start with facet events (facet-scoped), then expand to knowledge graph with strict filtering, filtering for facet relevance
5. Extract entities with day-specific context that are relevant to THIS facet, applying priority filters:
   - ALL people (highest priority) encountered in this facet's activities
   - SELECTIVE companies/organizations/projects (only important/central to this facet)
   - RARE tools/resources (only actively discussed in this facet's context)
6. Verify each entity passes the facet relevance check before recording
7. Record each entity using `sol call entities detect` for THIS facet
8. Call `emit_final(content=<detection counts by type + names>)` exactly once. Include detection counts by type and the names detected. If no detections were appropriate, call `emit_final(content="0 detections - facet was quiet or no entities passed facet relevance checks.")`

Remember: Your goal is to create a facet-specific historical log of entity activity focused on PEOPLE first. Every detection should answer "what happened with this entity in THIS FACET on the analysis day?" **Only detect entities that actively participated in this facet's work.** If a facet was quiet, 0 detections is correct. Cross-facet contamination is worse than under-detection. Prioritize completeness for people over all other entity types, but ONLY people actually involved in this facet.
