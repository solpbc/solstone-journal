---
name: support
description: >
  Draft support tickets and feedback to solstone support, search the KB, check open
  tickets and closed history, close tickets, confirm resolutions, check announcements,
  and run diagnostics. TRIGGER: file bug, request feature, submit feedback, search KB,
  announcements, tickets, close ticket, confirm resolution, still need help, closed
  history, sol call support create/search/list/reply/diagnose.
---

# sol support

Draft tickets, search the knowledge base, prepare feedback, and check existing support threads. Invoke via Bash: `sol call support <command> [flags]`.

## Before You Start

1. **Read the TOS first.** The TOS is cached locally at `<journal_root>/apps/support/portal/tos.txt` after first registration. If it does not exist yet, run `sol call support register` to fetch and cache it.

2. **Always search the KB before filing a ticket.** Run `sol call support search "your question"` first. Many common issues are already documented. Only prepare a ticket draft if the KB does not answer the question.

3. **Diagnostics are auto-populated.** When creating a ticket, `sol call support create` includes system info (version, OS, services, recent errors). You do not need to gather this manually.

4. **Draft only.** `create` and `feedback` run as dry-runs that save a local draft for owner review. `reply` and `attach` need `--no-submit` to draft. The draft stays local until the owner chooses from the review card.

## Subcommands

### Registration

```bash
sol call support register
```

Register (or re-register) with the support portal. Generates an RSA-4096 keypair on first use, signs the TOS, and creates an account. Run this if you get auth errors.

### Knowledge Base

```bash
# Search articles
sol call support search "transcription errors"

# Read a specific article
sol call support article getting-started
```

Always search before preparing a ticket draft. Present matching articles to the owner.

### Filing a Ticket

```bash
sol call support create \
  --subject "Transcription fails on long recordings" \
  --description "Recordings over 2 hours consistently fail with timeout errors. Started after updating to v2.1." \
  --severity medium \
  --category bug
```

The `create` command is a dry run by default: it prints the would-be payload and saves a local draft for owner review.

The `create` command implements a KB-first flow:
1. Searches KB for related articles
2. Shows matches
3. Includes diagnostics automatically
4. Shows the full ticket draft for review
5. Saves a local draft for the owner to review

**Flags:**
- `--subject` / `-s` - Ticket subject (required)
- `--description` / `-d` - Detailed description (required)
- `--product` / `-p` - Product name (default: solstone)
- `--severity` - low, medium, high, critical (default: medium)
- `--category` - bug, feature, question, account
- `--skip-kb` - Skip KB search (not recommended)
- `--anonymous` - Strip installation identifiers

### Ticket Management

```bash
# List open tickets
sol call support list

# List all tickets (including resolved)
sol call support list --status resolved

# View a ticket with thread
sol call support show 42

# Draft a reply to a ticket
sol call support reply 42 --body "Here's the additional info you requested..." --no-submit

# JSON output for any command
sol call support list --json
sol call support show 42 --json
```

### Closed History

```bash
# List closed tickets
sol call support history

# Continue from a previous page's cursor
sol call support history --cursor "<cursor>"
```

Closed history lists tickets that are no longer open. Use the returned cursor with
`--cursor` to continue to the next page.

### Ticket Lifecycle

```bash
# Prepare a draft to close a ticket
sol call support close 42

# Prepare a draft to accept a proposed resolution and close the ticket
sol call support resolved 42

# Prepare a draft to reject a proposed resolution and keep the ticket open
sol call support still-need-help 42
```

These commands are dry runs by default and save a local draft for owner review,
just like `create` and `feedback`. `close` ends the ticket, `resolved` accepts a
proposed resolution and closes it, and `still-need-help` rejects a proposed
resolution and keeps the ticket open. The review card decides whether anything
goes to solstone support.

### Attachments

```bash
sol call support attach 42 screenshot.png --no-submit
```

Screenshots can help support understand visual bugs. Prepare an attachment draft only when the owner explicitly provides or asks to attach a file, and always include `--no-submit`. Never attach journal content — transcript, screenshot, or journal-derived content — unless the owner explicitly asks. The attachment draft stays local until the owner chooses from the review card.

### Feedback

```bash
sol call support feedback --body "The entity search is great but I wish it could filter by date range"
```

Lower friction than a full ticket. Feedback is dry run by default: it prints the would-be payload and saves a local draft for owner review. Feedback is shaped as a ticket with category "feedback". Supports `--anonymous`.

### Announcements

```bash
sol call support announcements
```

Check for product updates, known issues, and maintenance notices.

### Local Diagnostics

```bash
sol call support diagnose
sol call support diagnose --json
```

Reflects the journal host (read-only; does not create a support ticket). Shows:
- solstone version
- OS/platform info
- Active services and their status
- Recent errors from service logs
- Configuration (secrets stripped)

## Good Ticket Descriptions

A good ticket includes:
- **What happened** - specific behavior observed
- **What was expected** - what should have happened
- **Steps to reproduce** - how to trigger the issue
- **Context** - when it started, how often, any recent changes

Version, OS, and service status are included automatically. You do not need to include these in the description unless they clarify the issue.

## Examples

```bash
# Owner reports a bug - check KB first
sol call support search "calendar sync"

# Prepare a ticket draft
sol call support create \
  --subject "Calendar events not syncing" \
  --description "Google Calendar events imported yesterday aren't showing up in the calendar app. Tried re-importing but same result." \
  --category bug \
  --severity medium

# Owner wants to give feedback
sol call support feedback \
  --body "Love the entity detection but it sometimes misidentifies project names as people"

# Draft a reply to an open ticket
sol call support reply 15 --body "The issue still reproduces after restarting convey." --no-submit

# Check for responses on open tickets
sol call support list
sol call support show 15

# Quick system health check
sol call support diagnose
```

Running `create` or `feedback` produces a safe dry-run preview and saves a local draft for owner review.

## Gotchas

- **`create`/`feedback` are dry-run by default.** They save a local draft for owner review. The `DRY RUN` banner in stdout is the signal that the draft remains local.
- **`close`/`resolved`/`still-need-help` are dry-run by default.** They also save a local draft for owner review with no flag needed.
- **`reply` and `attach` need `--no-submit` for drafts.** Always include it when preparing a reply or attachment for owner review.
- **KB-first is automatic on `create`.** The `create` command always searches the KB and shows matches for owner review before preparing the draft. Pass `--skip-kb` only if the issue is clearly unique.
- **`--product` defaults to solstone.** Solstone support handles other products too. Confirm with the owner before preparing a non-solstone ticket.
- **Diagnostics can include configuration.** Secrets are stripped, but the full diagnostic payload must still be shown to the owner.
