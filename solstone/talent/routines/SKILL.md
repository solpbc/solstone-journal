---
name: routines
description: >
  Recurring routines — daily briefings, weekly reviews, domain watches,
  commitment audits, meeting prep, or custom automations. Create from
  templates, adjust timing, pause/resume, run now, respond to suggestions.
  TRIGGER: routine, schedule, recurring, automate, daily brief, weekly
  review, journal routines create/list/edit/run.
---

# Routines CLI Skill

Manage recurring routines. Invoke via Bash: `journal routines <command> [args...]`. Never expose cron syntax, UUIDs, or CLI internals to the owner.

## Template guidance

| Template | When to propose | Default timing | What to ask about |
|----------|----------------|----------------|-------------------|
| `morning-briefing` | Wants a daily digest, morning summary, or "what's on my plate today" | Every morning at 7am | Which facets to include |
| `weekly-review` | Wants a weekly recap, reflection, or "how did my week go" | Friday evening | Which facets to cover, preferred day/time |
| `domain-watch` | Wants to track a topic, project, or area over time | Monday morning | Which domains/topics to watch, which facets |
| `relationship-pulse` | Wants to stay on top of key relationships or "who haven't I talked to" | Monday morning | Which facets, which relationships matter most |
| `commitment-audit` | Wants to catch dropped commitments, overdue items, or stale follow-ups | Monday morning | Which facets to audit |
| `monthly-patterns` | Wants a monthly retrospective or trend analysis | First of the month, morning | Which facets, what patterns matter |
| `meeting-prep` | Wants briefings before meetings — "prep me before each meeting" | 30 minutes before each calendar event | Which facets to draw context from |

Meeting-prep is event-triggered, not clock-scheduled. Explain this naturally: "It runs 30 minutes before each meeting on your calendar."

## Recognition

Notice when the owner is asking for a routine, even when they don't use that word:

- **Explicit scheduling:** "every morning, summarize my calendar" / "weekly, check in on the Acme deal"
- **Frustration with repetition:** "I keep forgetting to review my todos on Friday" / "I always lose track of follow-ups"
- **Direct request:** "set up a routine" / "can you do this automatically?"

## Creation conversation

When you recognize routine intent, guide the owner through creation:

1. **Propose a fit.** If a template matches, name it and describe what it does in plain language. If not, offer to build a custom routine.
2. **Confirm scope.** What facets should it cover? Default to all unless the intent clearly targets one area.
3. **Confirm timing.** Propose the template default in the owner's terms ("every morning at 7am", "Friday evening"). Let them adjust.
4. **Confirm timezone.** Default to the owner's local timezone from journal config. Only ask if ambiguous.
5. **Create and confirm.** Run the command, then confirm with a one-liner: "Done — your morning briefing will run daily at 7am."

Always set `--timezone` to the owner's local timezone when creating routines, not UTC.

## Custom routines

When no template fits, build a custom routine:

1. Ask the owner to describe what they want in plain language.
2. Draft a name, cadence in human terms, and instruction summary. Confirm with the owner.
3. Create it with explicit `--name`, `--instruction`, and `--cadence` flags.

## Command reference

| Intent | Command |
|--------|---------|
| Create from template | `journal routines create --template {template} --timezone {tz}` (add `--facets`, `--cadence` if overridden) |
| Create custom | `journal routines create --name "{name}" --instruction "{instruction}" --cadence "{cron}" --timezone {tz}` (add `--facets` if specified) |
| List all | `journal routines list` |
| Show templates | `journal routines templates` |
| Pause | `journal routines edit {name} --enabled false` |
| Resume | `journal routines edit {name} --enabled true` |
| Pause until date | `journal routines edit {name} --enabled false --resume-date {YYYY-MM-DD}` |
| Change cadence | `journal routines edit {name} --cadence "{cron}"` |
| Change facets | `journal routines edit {name} --facets "{comma-separated}"` |
| Change instruction | `journal routines edit {name} --instruction "{new instruction}"` |
| Delete | `journal routines delete {name}` |
| Run immediately | `journal routines run {name}` |
| Read output | `journal routines output {name}` (add `--date YYYY-MM-DD` for a specific day) |
| Toggle suggestions | `journal routines suggestions --enable` or `journal routines suggestions --disable` |
| Record response to a suggestion | `journal routines suggest-respond {template} --accepted` or `--declined` |
| Show suggestion state | `journal routines suggest-state` |

Use the routine's name for identification, never UUIDs.

## Management intents

Handle routine management conversationally. The owner says what they want; you translate using the Command reference table above.

- When the owner says "pause my morning briefing" or "stop the weekly review for now," you want to disable the routine. See Command reference for the exact edit form.
- When the owner says "turn my briefing back on" or "resume the weekly review," you want to re-enable it. See Command reference for the exact edit form.
- When the owner says "pause it until Monday," you want to disable it with a resume date. See Command reference for the exact edit form.
- When the owner says "move my briefing to 8am" or "make the review run on Sunday," you want to change the cadence. See Command reference for the exact edit form.
- When the owner says "add the work facet to my briefing" or "change the instruction to include...," you want to update facets or instruction. See Command reference for the exact edit forms.
- When the owner says "I don't need the weekly review anymore" or "remove that routine," you want to delete it after confirming. See Command reference for the exact delete form.
- When the owner says "what routines do I have?", you want to list routines and their status. See Command reference for the exact list form.
- When the owner says "what did my morning briefing say today?" or "show me last week's review," you want to read routine output. See Command reference for the exact output form.
- When the owner says "run my briefing now" or "do the weekly review right now," you want immediate execution. See Command reference for the exact run form.
- When the owner says "stop suggesting routines" or "turn routine suggestions back on," you want to toggle routine suggestions. See Command reference for the exact suggestions form.

## Tone

- Treat routines like setting an alarm — workmanlike, not ceremonial. "Done — morning briefing starts tomorrow at 7am."
- Never explain how routines work internally. The owner doesn't need to know about cron, agents, or output files.
- When the owner asks about routine output, present it as your own knowledge: "Your morning briefing found three meetings today and two overdue follow-ups."

## Pre-hook context

$active_routines

When active routines appear above, they list each routine's name, cadence, status, and recent output summary.

Use this to:
- Answer "what routines do I have?" without running a command
- Reference recent routine output naturally: "Your weekly review from Friday noted..."
- Notice when a routine is paused and offer to resume it if relevant

When no routines appear above, the owner has no routines yet. Don't mention routines proactively — wait for the owner to express a need.

## Progressive Discovery

$routine_suggestion

When a routine suggestion appears above, the owner's behavior matches a routine template. You did not request it — it was injected automatically.

**How to handle:**
- Read the pattern description to understand why the suggestion is relevant
- Mention it ONCE, naturally, at the end of your response — never lead with it
- Frame as an observation: "I've noticed this comes up often — would a routine help?"
- If the owner declines or shows no interest, drop it immediately. Do not bring it up again this conversation.
- After the owner responds, record the outcome:
  - Accepted: `journal routines suggest-respond {template} --accepted`
  - Declined: `journal routines suggest-respond {template} --declined`

**Never:**
- Suggest a routine without the eligible section in your context
- Push a suggestion after the owner declines or ignores it
- Mention the progressive discovery system or how suggestions work internally

## Responding to suggestions

When the system surfaces a routine suggestion and the owner accepts or declines it, record their response so the suggestion engine doesn't re-surface the same template prematurely:

```bash
journal routines suggest-respond morning-briefing --accepted
journal routines suggest-respond weekly-review --declined
```

Exactly one of `--accepted` or `--declined` is required. `suggest-state` prints the full JSON state for all templates if you need to inspect what the engine already knows.

## Gotchas

- **Timezone must be an IANA name.** `--timezone America/Denver` works; `--timezone MDT` does not. The CLI rejects the latter with a terse error.
- **Suggestion responses are idempotent within a day.** Calling `suggest-respond` twice in the same day overwrites the previous response. Don't loop on it.
