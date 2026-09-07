{
  "type": "generate",
  "title": "Facet Newsletter Generator",
  "description": "Creates comprehensive daily newsletters for each facet, covering activities, progress, and insights",
  "color": "#0d47a1",
  "schedule": "daily",
  "priority": 40,
  "output": "md",
  "accumulate": true,
  "hook": {"pre": "facet_newsletter", "post": "facet_newsletter"},
  "multi_facet": true
}

# Facet Newsletter: $facet for $day

## Source Packet

Reference only. Use this packet to ground the newsletter. Do not reproduce this section, the coverage notes, source counts, gaps, provenance, or metadata in the final output.

### Coverage

$coverage_preamble

### Source Counts

$source_counts

### Source Gaps

$source_gaps

### Packet

$source_packet

## Write

Write a clean owner-facing markdown newsletter for the `$facet` facet on `$day`.

Start with a short TL;DR. Then organize the body by project, thread, or theme rather than chronology. Name people in **bold** and describe what they contributed when the packet supports it. Include concrete details such as metrics, quotes, amounts, commit-like references, and dates only when they appear in the packet.

Cover decisions, action plans, followups, and next horizons when present. Use the prior newsletter only for continuity. Use facet metadata and entity context only for framing. Connect the day back to the facet's goals where the packet gives enough evidence.

Omit empty sections. If the packet supports only a short newsletter, write a short newsletter.

**Voice.** In the text you write, refer to the journal owner in second person ("you", "your"), never as "the user", "the owner", "this person", or in the third person. The software never speaks as "I", "we" or "my". Write the product name in lowercase: "solstone", never "Solstone". State what happened plainly. Never write "capture" in any form; never say the software watches, observes, records, monitors, tracks, listens, sees, hears or surveils, in any voice, including the passive ("was recorded", "were captured"); attach every claim to what the journal holds.

## Hard Rules

- The packet is complete.
- Synthesize only from the packet.
- Never recall or invent facts.
- Output clean markdown prose only.
- Do not include YAML frontmatter.
- Do not include sources, gaps, coverage, provenance, source counts, metadata blocks, generated/model lines, or implementation notes.
