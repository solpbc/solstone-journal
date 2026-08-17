{
  "type": "generate",

  "title": "Entity Description",
  "description": "Research and generate single-sentence descriptions for attached entities",
  "color": "#26a69a",
  "group": "Entities",
  "output": "md",
  "hook": {"pre": "entities:entity_describe"}
}

Generate a clear, informative single-sentence description for an attached entity.

## Input Context

- Entity Type: $entity_type
- Entity Name: $entity_name
- Facet: $facet
- Current Description: $current_description

## Journal Evidence

$evidence

## Description Guidelines

**Format:**
- Single complete sentence, under 100 characters preferred
- No quotes around the description
- Present tense for active entities, past tense for historical entities
- Return only the description sentence, with no preamble, markdown, or explanation

**Content by type:**

- **Person**: Role + relationship/context
  - "Senior backend engineer leading the API migration project"
  - "Friend from college, works in climate tech"

- **Company**: Industry + relationship
  - "AI research company, creator of Claude"
  - "Healthcare consulting client since Q3 2024"

- **Project**: Purpose + status/scope
  - "Internal tool for automated log analysis"
  - "Mobile app redesign initiative for Q1 launch"

- **Tool**: Category + use case
  - "Infrastructure-as-code framework for AWS deployments"
  - "Time-series database for metrics storage"

**If no journal evidence is found:**
- Use the entity type, entity name, facet, and current description
- Produce a generic but useful sentence
- Never leave the response empty

Return only one plain sentence.
