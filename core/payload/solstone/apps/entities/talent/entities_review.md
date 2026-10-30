{
  "type": "generate",
  "title": "entity reviewer",
  "description": "reviews detected entities and promotes recurring ones to attached status",
  "color": "#00796b",
  "schedule": "daily",
  "priority": 56,
  "multi_facet": true,
  "group": "Entities",
  "output": "json",
  "schema": "entities_review.schema.json",
  "hook": {"pre": "entities:entities_review", "post": "entities:entities_review"},
  "load": {"transcripts": false, "percepts": false, "talents": false},
  "max_output_tokens": 6144
}

## Your Job

You are given people and things that recurred across recent days in one area of the owner's life, and numbered pairs of names that might be two spellings of one thing. Your job is judgment only: decide which candidates become saved entities, write each one a lasting description, and decide which name pairs are truly one thing.

## What You're Given

$review_packet

## Candidates

Each candidate under "Recurring candidates" gets exactly one decision. Promote it when the days show a clear, specific person, company, project, tool or other named thing that will help future context in this area. Decline it when it is a generic word or category, a placeholder, a role or title with no name, or when the days contradict each other about what it is.

A promoted candidate's description says what it is and why it matters in this area, in one or two plain sentences that will still be true next year. Write only what the days agree on, in their terms: name a role, employer or relationship only when the notes state it, and leave out one named on one day and contradicted on another. Fold repeated contexts into one statement and leave out dates, single events and what happened in a particular session. A declined candidate's description is a few words on why.

An alias is another way of writing the same name: a nickname, initials, an acronym, a short form or a spelling variant that people use for exactly this candidate. A longer name that adds a word, such as a product, version, team, place, feature or a person's other name, names something else and is not an alias. Leave aliases empty unless a day shows the alternate form used for this candidate.

## Name Pairs

Each numbered line under "Possible name variants" holds two names in quotes, each followed by the kind of thing it was most often noted as when that is known. Most pairs are different things that share a word. Record a merge only when both names refer to exactly the same person or thing, so either could replace the other in every sentence. When one name adds a word that narrows it (a product of a company, a part of a project, a version of a tool, a city of an event) they are different things: record nothing for that pair. Names noted as different kinds (a company and a tool, a project and a person) are usually different things too.

For a merge, `pair` is the pair's number and `keep` is the one of its two names to keep, copied exactly without its quotes: for a person, the fuller proper name; for an organization, the everyday name without suffixes like Inc, LLC or Corp; otherwise the more common form. `evidence` is a short phrase saying why they are the same.

## What To Return

Return exactly one JSON object with `promotions` and `merges`.

`promotions` holds one entry for each candidate heading, in the order given, and nothing else (an empty array when the candidates section says None): `name` is the heading exactly as written, `promote` is true or false, `description` follows the rules above, and `aliases` is an array (empty when there are none).

`merges` holds only the pairs that are the same thing, and is an empty array when none are.
