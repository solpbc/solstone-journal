# partner

Behavioral profile of the journal owner — observed patterns that help sol
adapt its responses, timing, and initiative to how this person actually works.

## getting started

Everything stays on your machine — this journal is yours alone, never sent to sol pbc.

When meeting the owner for the first time, learn about them naturally through conversation.
Present one thing at a time — don't overwhelm.

### learn their name

Ask what they'd like to be called. Record it:
- `sol call sol set-owner "NAME"`
- With context: `sol call sol set-owner "NAME" --bio "SHORT_BIO"`

As you learn about them, update your partner profile:
- `journal identity partner --update-section 'SECTION' --value 'what you observed'`

### set up facets

Ask what areas of their life they want to track (work, personal, hobbies, side projects, etc.). Create facets for each:
- `sol call journal facet create TITLE [--emoji EMOJI] [--color COLOR] [--description DESC]`
- `sol call journal facets` — verify what was created

### attach entities

For each facet, ask about key people, companies, projects, and tools:
- `sol call entities attach TYPE ENTITY DESCRIPTION --facet FACET`
- Types: Person, Company, Project, Tool

### offer imports

After setup, offer to bring in history from existing tools:
- Calendar (ics), ChatGPT (chatgpt), Claude (claude), Gemini (gemini), Granola (granola), Notes (obsidian), Kindle (kindle)
- Read guide: `solstone/apps/import/guides/{source}.md`
- Navigate: `journal navigate "/app/import#guide/{source}"`
- If declined: `sol call awareness imports --declined`

### support

If the owner needs help or wants to share feedback, handle it in-place — file tickets, track
responses. Nothing gets sent without their review.

## work patterns
[not yet observed — sol will learn as we spend time together]

## communication style
[not yet observed — sol will learn as we spend time together]

## relationship priorities
[not yet observed — sol will learn as we spend time together]

## decision style
[not yet observed — sol will learn as we spend time together]

## expertise domains
[not yet observed — sol will learn as we spend time together]
