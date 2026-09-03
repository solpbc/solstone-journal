{
  "type": "generate",
  "title": "Segment Summary",
  "description": "Names the single most important event in each segment for the timeline.",
  "schedule": "segment",
  "priority": 41,
  "thinking_budget": 0,
  "max_output_tokens": 1024,
  "output": "json",
  "schema": "segment_summary.schema.json",
  "hook": {"pre": "timeline:segment_summary", "post": "timeline:segment_summary"},
  "load": {"transcripts": false, "percepts": false, "talents": false}
}

Pick the SINGLE MOST IMPORTANT EVENT from this ~5-minute slice of a personal life-journal and name it. The output is one cell in a multi-scale timeline UI — each cell shows a 2-line title and a 3-line description, so brevity matters more than completeness.

An EVENT is a discrete thing that happened: a decision made, a problem solved, a message sent, a system change applied, a person met, a file shipped, a milestone reached. NOT a topic area, NOT a feeling, NOT a generic activity descriptor.

Anti-patterns to avoid:
  BAD: 'Coding Session' (topic, not event)
  BAD: 'Working on KDE' (activity descriptor)
  BAD: 'System Maintenance' (generic)
  GOOD: 'GDM Service Restart' (specific event)
  GOOD: 'Trademark Filed' (discrete action)
  GOOD: 'Crash Diagnosed' (concrete result)

If multiple noteworthy events occurred, pick the one with the highest consequence — a decision over a routine action, a fix over an investigation, a shipped artifact over a draft.

FIELD RULES (hard caps):
- title: max 3 words, max 22 characters, headline case. Name the EVENT as a noun phrase or past-tense action. Shorthand is encouraged: Dev, Env, Config, UI, App, Repo, PR, Bug, Cli, Doc, Auth, K8s, KDE, GDM, Wallet, Plasma, GNOME. Drop articles. Prefer specific over generic ('KDE Panel Fix' beats 'System Config'). Examples: 'Display Reset', 'Trademark Filed', 'Dev Env Debug', 'Sprint Planned', 'Crash Triage'.
- description: max 10 words, max 60 characters, ONE sentence, third person, present tense, verb-led, describing what happened in/around the event. Examples: 'Restarts display manager to recover desktop session.' (51c) 'Files trademark application with specimen images.' (49c) 'Identifies GNOME DBus dependency causing the crash.' (52c). No first-person ('I', 'me'). No times ('17:02'). No segment IDs.

If the input is empty or trivial, still return a plausible compact {title, description} for whatever did happen.

Segment: $segment_rel_path

Activity summary for this slice:
---
$activity_text
---

Return JSON {title, description} per the system instruction.
