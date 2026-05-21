# Field Journal — Public-Domain Dev Mode

`setup_field_journal.sh` (at the repo root) populates `journal/chronicle/` with content from [solpbc/field_journal](https://github.com/solpbc/field_journal) — a curated public-domain set of audio and screen recordings — so an instance of solstone runs against real, reproducible test material instead of personal capture data.

This is an **opt-in dev/test primitive**. It is not part of the canonical install or setup paths (`make install` and `sol setup` do not invoke it, and the script is deliberately not wired into the Makefile). Reach for it when you want:

- a contributor or contractor on solstone who shouldn't be exposed to a maintainer's personal journal,
- an integration-test scenario seeded from stable, redistributable media,
- a clean dev environment for exercising the full observe → think → convey pipeline against real media without recording your own day first.

It is **not** a path you'd use on a personal-capture journal — see [running against an existing personal journal](#running-against-an-existing-personal-journal) below if you need to switch.


## One-time setup

### 1. Clone field_journal

```sh
git clone https://github.com/solpbc/field_journal ~/Field_Journal
```

`~/Field_Journal/` is read-only from solstone's perspective. Never commit, push, or let solstone write into it — the setup script exists specifically to avoid that by copying rather than symlinking.

### 2. Scaffold the journal

If you don't already have a configured `journal/` (identity, providers, convey secret, facets), bootstrap one the normal way first — `make install` (source checkout) or `uv tool install solstone` (packaged) followed by `sol setup`, then whatever initial first-run wizard work brings the journal to a usable state. `setup_field_journal.sh` only populates `chronicle/`; it expects the rest of the journal scaffolding to already exist.

### 3. Populate chronicle from field_journal

```sh
./setup_field_journal.sh
```

Copies each `YYYYMMDD` day directory from `~/Field_Journal/journal/` into `journal/chronicle/`. Copies (not symlinks) — solstone writes derived artifacts (`audio.jsonl`, `audio.npz`, screen descriptions, etc.) as siblings of source media, and symlinking would dirty the field_journal clone.

Options:

- `--source PATH` — field_journal clone location (default: `~/Field_Journal`).
- `--force` — overwrite chronicle days that already exist.

After populating chronicle, the journal is ready for `sol setup` (if you haven't run it yet) or the running pipeline.


## Running against an existing personal journal

If your `journal/` already holds personal capture data and you want to switch to the field_journal corpus, back it up first:

```sh
mv journal journal.bak-$(date +%Y%m%d)
```

Then recreate the structural parts (config, identity, facets skeleton, tokens, link state, routines) in a fresh `journal/`, either by copying from the backup or by re-running setup. Do not carry over `chronicle/`, `indexer/`, `entities/`, or `health/` — those are derived and will be regenerated from field_journal media.


## Refreshing after upstream updates

```sh
git -C ~/Field_Journal pull
./setup_field_journal.sh --force
```

`--force` replaces each day wholesale, including any derived artifacts solstone wrote under it. After a force-refresh, rerun the pipeline to rebuild derived state from the new media.


## Running the pipeline

field_journal provides **media only** — transcripts, descriptions, entities, facets, and indexer state are not pre-built. After populating chronicle, run the stack (`sol up` / `make dev`) to have the think-side produce derived artifacts from the new media.


## Stream naming

field_journal uses `field.audio` and `field.screen` as stream names. These are compatible with solstone as-is: the stream-name validator accepts dotted names, and downstream processing (sense, transcribe, describe) dispatches on file extension rather than stream name. No rename step is required.
