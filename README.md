<img src="docs/static/sol-wordmark.svg" alt="solstone" width="300">

# solstone journal

your co-brain — observers experience your day along with you, sol curates your memories, and your journal holds everything.

the python core of the solstone product family — the journal layer that the [solstone native apps](https://solstone.app) wrap. it runs in the background on your computer, experiencing your day along with you. AI agents transcribe, extract entities, detect meetings, build knowledge graphs, and surface daily insights — all without any manual input. everything stays on your machine in daily journal directories. open source, local-first, no cloud required.

Python 3.11+, Linux + macOS, AGPL-3.0-only, maintained by [sol pbc](https://solpbc.org).

<img src="docs/static/screenshot-home.png" alt="solstone daily dashboard" width="800">

*Daily dashboard — goal, todos, upcoming events, and detected entities, all generated from observations. Facet tabs organize your life by project or context.*

## what you get

**a system of intelligence, not just a system of record.**

- **automatic transcription** — standalone observers take in audio continuously with speaker identification. every conversation, transcribed and searchable.
- **people and projects** — extracted from your conversations and remembered across time.
- **knowledge graphs** — relationships between entities mapped automatically. who works with whom, which projects connect to which people.
- **meeting detection** — meetings identified, summarized, and linked. meeting prep that surfaces what you discussed last time and personal context you'd forget.
- **commitments** — todos extracted from natural conversation. no manual entry.
- **facet organization** — group everything by project or context (work, personal, client-name) with scoped views across all apps.
- **AI chat** — talk to your journal. ask anything about your digital life and get answers grounded in your actual data.
- **full-text search** — find anything you've ever seen or heard.
- **30 AI agents** — configurable workflows for activities, scheduling, research, media analysis, and more. extensible via the agent skill framework.
- **local-first** — all data in daily journal directories on your filesystem. configurable AI providers (Google Gemini, OpenAI, Anthropic). no cloud dependency.

<img src="docs/static/screenshot-transcripts.png" alt="solstone transcript viewer" width="800">

*Transcript viewer — dual-timeline navigation, speaker-diarized dialogue, audio playback, screen analysis. every conversation browsable by time.*

<img src="docs/static/screenshot-entities.png" alt="solstone people and projects" width="800">

*People and projects — automatically extracted and remembered across your journal with mention counts and relationship data.*

## architecture

```text
  +---------+       +----------------+       +---------+
  | observe | ----> |    journal     | ----> |  think  |
  | capture |       | YYYYMMDD/ dirs |       | process |
  +---------+       | media, jsonl,  |       | index   |
                    | entities       |       +----+----+
                    +-------+--------+            |
                            ^                     |
                            |  agent outputs      |
                       +----+----+                |
                       | cortex  | <--------------+
                       | agents  |
                       +---------+
                            |
  ==== callosum (event bus) | ==========================
                            |
                     +------+------+
                     |   convey    |
                     | web UI      |
                     +-------------+
```

- **observe** — receives audio and screen observations from standalone observers (solstone-linux, solstone-tmux, solstone-macos) via observer ingest. processes FLAC audio, WebM screen media, and timestamped metadata.
- **think** — transcribes audio (faster-whisper), analyzes screen observations, surfaces entities, detects meetings, and indexes everything into SQLite. runs 30 configurable agent/generator templates from `solstone/talent/`.
- **cortex** — orchestrates agent execution. receives events, dispatches agents, writes results back to the journal.
- **callosum** — async message bus connecting all services. enables event-driven coordination between observe, think, cortex, and convey.
- **convey** — Flask-based web interface with 17 pluggable apps for navigating journal data.
- **journal** — `journal/YYYYMMDD/` daily directories. the single source of truth — transcripts, media, entities, agent outputs, and the SQLite index all live here.

## quick start

```bash
uv tool install solstone
journal setup
```

(or `pipx install solstone && journal setup`.)

then open http://localhost:5015 in a browser; the first-run wizard handles identity and the gemini API key. network access, and the password it requires, can be configured later in settings → security.

see [INSTALL.md](INSTALL.md) for prerequisites, observer install, and troubleshooting; see [CONTRIBUTING.md](CONTRIBUTING.md) if you want to develop on solstone from a source checkout.

## CLI

solstone is operated through `sol` for day-to-day journal access and `journal` for host operations.

```bash
sol                    # Status overview and command list
journal supervisor         # Start the full stack (capture + processing + web)
sol chat               # Interactive AI chat from the terminal
journal transcribe <file>  # Transcribe an audio file
sol indexer            # Rebuild the search index
```

Run `sol help` for the full command reference.

## documentation

| Topic | Document |
|-------|----------|
| Installation and setup | [INSTALL.md](INSTALL.md) |
| Developing from source | [CONTRIBUTING.md](CONTRIBUTING.md) |
| Journal structure and data model | [solstone/talent/journal/SKILL.md](solstone/talent/journal/SKILL.md) |
| Capture pipeline | [docs/OBSERVE.md](docs/OBSERVE.md) |
| Processing and agents | [docs/THINK.md](docs/THINK.md) |
| Web interface | [docs/CONVEY.md](docs/CONVEY.md) |
| App development | [docs/APPS.md](docs/APPS.md) |
| Agent runtime | [docs/CORTEX.md](docs/CORTEX.md) |
| Message bus | [docs/CALLOSUM.md](docs/CALLOSUM.md) |
| AI provider configuration | [docs/PROVIDERS.md](docs/PROVIDERS.md) |
| What solstone sends to your AI provider | [DATA-FLOW.md](DATA-FLOW.md) |
| Troubleshooting | [docs/DOCTOR.md](docs/DOCTOR.md) |
| Project direction | [docs/ROADMAP.md](docs/ROADMAP.md) |

## development

See [AGENTS.md](AGENTS.md) for development guidelines, coding standards, and testing instructions.

Use `make dev` to run the full stack against test fixtures and `make ci` for pre-commit checks.

## feedback

Questions, feedback, or a bug? **Follow and tag [@solstone.app](https://bsky.app/profile/solstone.app) on Bluesky** for discussion and updates, open an issue at [github.com/solpbc/solstone-journal/issues](https://github.com/solpbc/solstone-journal/issues) for bugs, or reach support at [support.solstone.app](https://support.solstone.app). You don't need to know anyone — those are the front doors.

## contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution terms.

## license

AGPL-3.0-only. See [LICENSE](LICENSE) for details.
Maintained by [sol pbc](https://solpbc.org).
