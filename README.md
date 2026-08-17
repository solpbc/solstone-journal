<img src="docs/static/sol-wordmark.svg" alt="solstone" width="300">

# the journal

a memory your agents can work from. sol, the app on your devices, experiences your day with you and keeps it in your journal. the journal lives on a computer you choose.

this repo is that journal, plus sol. the [sol apps](https://solstone.app) pair with a journal running on a machine you pick. sol transcribes, extracts entities, detects meetings, builds knowledge graphs, and surfaces daily insights, without you filing anything by hand. your journal is a folder of dated directories on that machine. open source, local-first. if you point sol at your own provider key, [DATA-FLOW.md](DATA-FLOW.md) says what leaves.

linux. mac runs the sol app today; the mac build of the journal is not published yet. AGPL-3.0-only, maintained by [sol pbc](https://solpbc.org).

<img src="docs/static/screenshot-home.png" alt="solstone daily dashboard" width="800">

*Daily dashboard: goal, todos, upcoming events, and detected entities, all generated from what sol kept. Facet tabs organize your life by project or context.*

## what you get

- **automatic transcription:** sol keeps conversations in your journal, transcribed with speaker identification and searchable.
- **people and projects:** extracted from your conversations and remembered across time.
- **knowledge graphs:** who works with whom, which projects connect to which people.
- **meeting detection:** meetings identified, summarized, and linked, with prep that surfaces what you discussed last time.
- **commitments:** todos extracted from natural conversation. no manual entry.
- **facet organization:** group everything by project or context (work, personal, a client name) with scoped views across all apps.
- **ask sol:** ask anything about your journal and get answers grounded in it.
- **full-text search:** find anything in your journal.
- **workflows:** scheduling, research, media analysis, and more, extensible via skills.
- **local-first:** a folder of dated directories on your machine. sol thinks locally by default; you can point it at your own provider key if you'd rather.

<img src="docs/static/screenshot-transcripts.png" alt="solstone transcript viewer" width="800">

*Transcript viewer: dual-timeline navigation, speaker-diarized dialogue, audio playback, screen analysis. every conversation browsable by time.*

<img src="docs/static/screenshot-entities.png" alt="solstone people and projects" width="800">

*People and projects: extracted and remembered across your journal with mention counts and relationship data.*

## architecture

```text
  +---------+       +----------------+       +---------+
  | observe | ----> |    journal     | ----> |  think  |
  | inputs  |       | YYYYMMDD/ dirs |       | process |
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

- **observe:** receives audio and screen from sol on your devices (solstone-linux, solstone-tmux, solstone-macos). processes FLAC audio, WebM screen media, and timestamped metadata.
- **think:** transcribes audio with Parakeet, analyzes screen, surfaces entities, detects meetings, and indexes everything into SQLite. runs talent templates from `core/payload/solstone/talent/`.
- **cortex:** orchestrates talent runs. receives events, dispatches work, writes results back to the journal.
- **callosum:** async message bus connecting all services.
- **convey:** web interface with pluggable apps for navigating journal data.
- **journal:** a folder of dated directories. transcripts, media, entities, talent outputs, and the SQLite index all live here.

## quick start

the journal ships as one self-contained tree. it needs no interpreter and no package manager of its own.

⚠ **the tree is not published yet.** its release channel is `updates.solstone.app`. until the first release lands there, start from a local build or a copy someone handed you. [INSTALL.md](INSTALL.md) is the full guide.

once it is published, one command does the whole thing:

```bash
sh install.sh --version <version>
```

that fetches the archive from `updates.solstone.app`, verifies the digest, and installs. today, with the files already on disk:

```bash
sh core/distribution/install.sh --archive solstone-journal-<version>-linux-x86_64.tar.gz \
              --sha256 solstone-journal-<version>-linux-x86_64.sha256 \
              --release solstone-journal-<version>-linux-x86_64.release
journal setup
```

debian and fedora can install the `.deb` or `.rpm` instead. one tree covers running the journal and talking to a journal that already runs elsewhere; there is no separate client download.

not sure a computer is up to running the journal? after the tree is on PATH, `sol check` gives a one-shot readiness verdict (gpu, memory, and disk) before you run setup.

then open http://localhost:5015 in a browser. the first-run wizard sets up your identity and gets sol thinking, locally by default, or on your own provider key if you'd rather.

if you still have a pip, uv or pipx install of the old journal packages, [INSTALL.md](INSTALL.md#moving-from-a-pip-uv-or-pipx-install) is the migration. there is no CUDA package of the tree.

see [INSTALL.md](INSTALL.md) for prerequisites, sol on your other devices, and troubleshooting. see [CONTRIBUTING.md](CONTRIBUTING.md) to develop on solstone from a source checkout.

## CLI

solstone is operated through `sol` for day-to-day journal access and `journal` for host operations.

```bash
sol                    # Status overview and command list
journal supervisor         # Start the full stack (observe + processing + web)
sol chat               # Interactive AI chat from the terminal
journal transcribe <file>  # Transcribe an audio file
journal indexer            # Rebuild the search index
```

Run `sol help` for the full command reference.

## documentation

| Topic | Document |
|-------|----------|
| Installation and setup | [INSTALL.md](INSTALL.md) |
| Developing from source | [CONTRIBUTING.md](CONTRIBUTING.md) |
| Journal structure and data model | [core/payload/solstone/talent/journal/SKILL.md](core/payload/solstone/talent/journal/SKILL.md) |
| Observe pipeline | [docs/OBSERVE.md](docs/OBSERVE.md) |
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

Use `make dev` to run the full stack against test fixtures, focused test targets
during development, and efficient `make ci` for routine validation. The
[Makefile](Makefile) defines both Rust gates; operators use `make ci-full` on the
exact final tree before merge or release.

## feedback

questions, feedback, or a bug: follow and tag [@solstone.app](https://bsky.app/profile/solstone.app) on bluesky, open an issue at [github.com/solpbc/solstone-journal/issues](https://github.com/solpbc/solstone-journal/issues), or reach [support.solstone.app](https://support.solstone.app).

## contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution terms.

## license

AGPL-3.0-only. See [LICENSE](LICENSE) for details.
Bundled third-party model notices: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
Maintained by [sol pbc](https://solpbc.org).
