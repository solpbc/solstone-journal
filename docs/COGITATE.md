# Cogitate runtime contract

The canonical contract for **cogitate talents** — the LLM agents Cortex spawns to
read and update the journal (chat, the entity / activity / import talents, and
the rest). It is the single place a talent
author can point at and say what a fresh cogitate run's working directory,
context, tools, finalization, and persistence are — instead of reverse-engineering
the runtime from `solstone/think/talents.py`, the providers, the policy, and the
installed OpenHands SDK.

This contract **names capabilities and access classes.** It does not define the
HTTP API shape (that is convey's — see `docs/CONVEY.md` and `docs/SOLCLI.md`) and
it is not a per-talent prompt. Every talent prompt is written against this
contract.

> **Source of truth for the in-context part:** the short runtime preamble every
> cogitate talent receives is the constant `COGITATE_RUNTIME_PREAMBLE` in
> `solstone/think/cogitate_contract.py`. The locked access-tier and finalization
> vocabularies live in the same module (`COGITATE_ACCESS_TIERS`,
> `FUTURE_ACCESS_TIERS`, `TALENT_FINALIZATION_MODES`). Reference those by path; do
> not paraphrase them.

---

## What a cogitate run is

A cogitate run is **not** a coding agent (Claude / Codex / Gemini) launched in the
journal. It is a Python talent subprocess spawned by Cortex, inside which the
OpenHands SDK builds an `Agent` and a `Conversation` and runs the model against a
small, explicit tool set.

```
Cortex (cortex/request)
   |- spawns: python -m solstone.think.talents      (cwd = journal root)
        |- prepare_config()                          (talents.py)
             |- run_cogitate()                        (providers/openhands.py)
                  |- assemble_prompt()                (providers/cli.py)
                  |- build the `sol` tool + CogitatePolicy
                  |- OpenHands Agent (system_prompt = preamble + instruction)
                  |- OpenHands Conversation -> model loop -> finalization
```

The model's initial context is **solstone-assembled prompt material plus the
OpenHands tool schemas** — not a snapshot of the cwd. Files that merely sit in the
working directory are not in context (see "What is *not* in a talent's context").

## Working directory and OpenHands workspace

These are two different things; conflating them is the most common debugging
misread.

| | Value | What it governs |
|---|---|---|
| **Talent subprocess cwd** | the **journal root** (resolved from `cwd: journal`) | the cwd the `sol` tool inherits — i.e. where `sol` / `sol call` commands run |
| **OpenHands `Conversation` workspace** | the **repo root** (`get_project_root()`) | SDK bookkeeping and file-based subagent discovery — **not** automatic model context |
| **Conversation persistence** | `journal/.cache/cogitate-history/<session_id>` | the SDK event/state store for the run |

The journal-root cwd matters operationally because the `sol` tool inherits it, but
**cwd contents are never automatically loaded into the model's context.**

## The `sol` CLI is the authoritative talent-to-journal contract

A talent reaches journal functionality by emitting `sol` / `sol call ...`
command lines. The approved host command families `journal identity ...`,
`journal health ...`, and `journal talent ...` also run directly and must not be
prefixed with `sol` or `sol call`. The runtime parses each tool call as one
command-line invocation and executes that argv directly; it is not an arbitrary
shell. The CLI handlers are the **single authoritative translation layer**
between a talent and the journal: they turn CLI syntax into journal operations.
A talent therefore:

- **never** talks to the convey HTTP API directly, and
- **never** assumes a database, a socket, or any journal access other than the
  documented command forms and the bounded raw-read tools below.

The tool call shape is `sol(command="sol call activities list")`, or
`sol(command="journal identity partner")` for an approved host command family.
The command policy (below) constrains what may run and rejects shell composition
such as pipes, redirects, chaining, and command substitution.

## Reads: domain reads vs raw evidence reads

**Domain reads — the default.** Indexed, normalized, or semantic reads go through
`sol call` verbs (e.g. `sol call activities list`, `sol call entities search`,
`sol call transcripts read`). Anything that needs authorization, derived state,
normalization, or audit behavior is a domain read, not a raw read.

**Raw evidence reads — bounded, for evidence with no `sol` verb.** A normal talent
may be given a small read tier (`read_file`, `list_directory`, `glob`,
`grep_search`) for raw evidence (e.g. a per-span narrative file) that no domain
command exposes. Reads resolve relative to wherever the caller points them —
there is no broad journal-root default — and are bounded by tool-specific scope
and limits:

- `glob` and directory `grep_search` (always recursive) refuse to scan
  recursively from the journal root, `chronicle/`, or `facets/` themselves —
  point them at a day, a facet tree, or another subdirectory below one of those;
  a `list_directory` call hits the same refusal only when `recursive=True`;
- a **denylist** is always refused: `.git`, caches (`.cache`), credentials,
  virtualenvs, and `node_modules`;
- **per-call and per-run caps** bound output size and the number of read calls;
- traversal and symlink escape are refused; binary files, directories, sockets,
  devices, and FIFOs are refused by `read_file`.

The journal is the owner's own data, so caps and the denylist are the safety
bound — not a directory-scope permission gate. An optional per-talent
`read_scope` is a **narrowing hint** only; it is not the default gate, and a
talent never fails for reading an undeclared journal file.

> The raw-read **tools** are built to this contract and registered per resolved
> access tier. Check the run's actual tool schema
> (`journal talent show <name> --prompt`) for what a specific talent receives.

## Writes: only through approved journal commands

A talent **writes journal state only through approved journal commands** (the
`sol call ...` verbs for the domain it is updating, e.g.
`sol call entities update ...`, plus approved direct host commands when a prompt
names one). There is **no general-purpose write tool** and no raw-write tier —
write-class tools are denied by policy. The mechanic is centralized in the owning
command; a talent gets the *capability* to update a domain, never an arbitrary
file-write path. Persistence that does not go through an approved journal command
does not happen.

## Access tiers

A talent's access class is one of the following. The tier vocabulary is locked
(`COGITATE_ACCESS_TIERS` / `FUTURE_ACCESS_TIERS` in
`solstone/think/cogitate_contract.py`); the per-talent assignment and its
enforcement are layered on top of it.

| Tier | Purpose | Surface |
|---|---|---|
| `normal` | default cogitate talents | the `sol` tool (`sol` / `sol call`, plus approved direct `journal` families when a prompt names one), the bounded raw-read tier, a finalization tool |
| `system-read` | diagnostics boundary for scoped operational evidence | no cogitate talent claims it today (steward was demoted to a deterministic renderer + `lite` generate); when used, the declared surface is the `sol` tool (`sol` / `sol call`, plus approved direct `journal` families when a prompt names one), the bounded raw-read tier, and a finalization tool, with scoped evidence arriving through a talent pre-hook rather than an extra model read tool |
| `outbound` | comms-like talents that may submit something that leaves the machine (e.g. `support`) | the `sol` tool (`sol` / `sol call`, plus approved direct `journal` families when a prompt names one) and a finalization tool, plus submit-capable support commands gated on per-send owner approval supplied only by a human-initiated chat launch; no raw-read tier — drafts and evidence go through `sol` domain commands |
| `synthesis` | pure command-surface synthesis talents (e.g. `weekly_reflection`, `partner`) whose source of record is a documented command form, not the raw journal tree | the `sol` tool (`sol` / `sol call`, plus approved direct `journal` families when a prompt names one) and a finalization tool; **no raw-read tier and no submit** — same as `outbound` minus the outbound submit capability. Removing the raw-read tools keeps a synthesis talent from spelunking `chronicle/` / `talents/` / `facets/` and burning its budget instead of using documented commands |

Policy denies support send verbs (`create`, `reply`, `attach`, `feedback`) for
`normal` / `system-read` runs. `outbound` runs may use those verbs only when the
launch config carries runtime owner send-approval. An agent cannot self-grant
approval through prompt text, static frontmatter, templates, or scheduled
injection.

**There is no `repair` tier in cogitate.** Health fact-gathering and repair run
through the deterministic `journal heartbeat` workflow, explicitly launched and
logged without an LLM. `steward` no longer runs as a cogitate talent at all: the
owner-facing `health.md` body is rendered **deterministically** in its pre-hook,
and a tiny `lite` **generate** talent writes only the human-friendly summaries
(`headline` / `summary_sentence` / a closed-enum `suggested_action`) the home
widget surfaces. No cogitate health surface remains.

`code-agent` is a **documented future tier**, not part of the current cogitate
runtime. A code agent needs write access, broad tools (read / edit / write / shell /
delegation), and a repo cwd — deliberately out of scope here. It gets its own clean
contract if and when it is built; it is listed in `FUTURE_ACCESS_TIERS` so nothing
keys enforcement off it today.

## Finalization

Every talent has exactly one finalization mode (`TALENT_FINALIZATION_MODES` in
`solstone/think/cogitate_contract.py`):

| Mode | When | Behavior |
|---|---|---|
| `emit_final` | scheduled / output talents (an `output_path` is set, or the schedule is `daily` / `weekly` / `activity`) | the run is accepted **only** from the `emit_final` tool; `FinishTool` is disabled; a finish with no emitted final is an error |
| `FinishTool` | manual / no-output talents | the run signals completion through OpenHands' built-in finish tool |
| `quiet` | side-effect-only talents | the talent has already persisted its work through approved journal commands and finishes with no output |

A manual talent's prompt should state which of these it expects so the behavior is
explicit rather than inferred.

## What is *not* in a talent's context (disallowed assumptions)

A talent prompt must not assume any of the following — they are named here so
prompts can be checked against them:

- **other bare `journal ...` families** (e.g. `journal search`, `journal facet`,
  `journal navigate`) — reach the journal through the documented `sol` /
  `sol call` form;
- **raw `cat` / `ls` / arbitrary shell file reads** — use the raw-read tools when
  present, not shell file access;
- **auto-loaded skills, `AGENTS.md`, `CLAUDE.md`, or `GEMINI.md`** — these are
  normal journal files with no special status. They are *not* injected into a
  talent's context; this contract is the talent's source of truth. (They remain
  affordances for coding agents launched in the journal, not for cogitate.)
- **browser / web tools, MCP tools, and sub-agent delegation** — none are bound;
- **`cwd: repo` / `write: true` modes** — not cogitate modes; a normal talent runs
  with the journal cwd and no write tool.

## The in-context preamble (named source constant)

Every cogitate run receives the contract's essential preamble as the head of its
system prompt. It is the constant `COGITATE_RUNTIME_PREAMBLE` in
`solstone/think/cogitate_contract.py`, prepended in `assemble_prompt`
(`solstone/think/providers/cli.py`) ahead of the talent's own instruction. The
verbatim text:

```
You are a solstone cogitate talent running inside the live system. This runtime contract is authoritative; do not assume capabilities beyond it.

- Reach the journal through the `sol` command line: emit `sol` / `sol call ...` command lines, e.g. sol(command="sol call activities list"). The runtime runs each call as a single parsed command-line invocation, not an arbitrary shell. The `sol` CLI is the one authoritative path between you and the journal; never assume direct database, socket, or HTTP access.
- The approved host command families are identity, health, talent; run them directly as `journal <family> ...` through the same tool, never prefixed with `sol` or `sol call`.
- Write journal state only through approved journal commands (`sol call ...` verbs for the data you own, plus approved direct host commands when a prompt names one). There is no general-purpose write tool; persistence that does not go through an approved journal command will not happen.
- Raw evidence reads use the provided read tools (`read_file`, `list_directory`, `glob`, `grep_search`) for bounded journal evidence: a denylist (`.git`, caches, credentials, virtualenvs, `node_modules`) and per-call / per-run caps apply. Recursive scans must not start at the journal root, `chronicle/`, or `facets/`: `glob` and directory `grep_search` must start below them, as must recursive `list_directory`. Prefer `sol call` reads; use raw reads only for evidence that has no `sol` command.
- Finalize as your run is configured: call `emit_final` when an `emit_final` tool is present; otherwise finish through the built-in finish tool; a side-effect-only talent that has already persisted its work finishes quietly with no output.
- Do not assume tools or context you were not given: no other bare `journal ...` family, no raw `cat` / `ls` / shell file reads, no shell composition (pipes, redirects, chaining, or command substitution; one command per call), no auto-loaded skills or AGENTS.md / CLAUDE.md, no browser or web access, no MCP tools, and no delegating to sub-agents. Any guidance file is a normal journal file with no special status; this contract is your source of truth.
```

## Where this is wired (for maintainers)

| Concern | Location |
|---|---|
| The contract preamble + locked vocabularies | `solstone/think/cogitate_contract.py` |
| Preamble injection into the system prompt | `assemble_prompt` in `solstone/think/providers/cli.py` |
| Cogitate run assembly (tools, policy, workspace, finalization) | `run_cogitate` in `solstone/think/providers/openhands.py` |
| Command / write policy | `CogitatePolicy` in `solstone/think/cogitate_policy.py` |
| Finalization tool | `solstone/think/providers/emit_final_tool.py` |
| Talent configs (prompts + frontmatter) | `solstone/talent/*.md`, `solstone/apps/*/talent/*.md` |
| Talent execution lifecycle | `docs/CORTEX.md`, `docs/THINK.md` |
