<img src="docs/static/mark.svg" alt="solstone" width="300">

# The journal

A memory your agents can work from. The solstone app takes in what you share with it, and all of it goes into your journal. The journal lives on a device you own.

This repo is that journal. You pair each [solstone app](https://solstone.app) with a journal running on a device you own. Once material is in your journal, you can work with transcriptions, entities, meetings, knowledge graphs, and daily insights without filing anything by hand. Your journal is a folder of dated directories on that device. Open source, local-first. If you choose your own provider key, [DATA-FLOW.md](DATA-FLOW.md) says what leaves.

linux, and macos on Apple Silicon. windows is not yet supported. AGPL-3.0-only, maintained by [sol pbc](https://solpbc.org).

<img src="docs/static/screenshot-home.png" alt="solstone daily dashboard" width="800">

*Daily dashboard: your goal, upcoming events, and detected entities from your journal. Facet tabs organize your life by project or context.*

## What you get

- **automatic transcription:** conversations you share with the solstone app go into your journal, transcribed with speaker identification and searchable.
- **people and projects:** extracted from your conversations and remembered across time.
- **knowledge graphs:** who works with whom, which projects connect to which people.
- **meeting detection:** meetings identified, summarized, and linked, with prep that surfaces what you discussed last time.
- **commitments:** detected from natural conversation and kept with their source context. No manual entry.
- **facet organization:** group everything by project or context (work, personal, a client name) with scoped views across all apps.
- **ask about your journal:** get answers grounded in it.
- **full-text search:** find anything in your journal.
- **workflows:** scheduling, research, media analysis, and more, extensible via skills.
- **local-first:** a folder of dated directories on your machine. You can choose your own provider key if you'd rather.

<img src="docs/static/screenshot-transcripts.png" alt="solstone transcript viewer" width="800">

*Transcript viewer: dual-timeline navigation, speaker-diarized dialogue, audio playback, screen analysis. Every conversation browsable by time.*

<img src="docs/static/screenshot-entities.png" alt="solstone people and projects" width="800">

*People and projects: extracted and remembered across your journal with mention counts and relationship data.*

## Architecture

```text
  +---------+       +----------------+       +---------+
  | intake  | ----> |    journal     | ----> |  think  |
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

- **intake:** accepts audio and screen material from the solstone apps on your devices (solstone-linux, solstone-tmux, solstone-macos). Processes FLAC audio, WebM screen media, and timestamped metadata.
- **think:** transcribes audio with Parakeet, analyzes screen, surfaces entities, detects meetings, and indexes everything into SQLite. Runs talent templates from `core/payload/solstone/talent/`.
- **cortex:** orchestrates talent runs. Receives events, dispatches work, writes results back to the journal.
- **callosum:** async message bus connecting all services.
- **convey:** web interface with pluggable apps for navigating journal data.
- **journal:** a folder of dated directories. Transcripts, media, entities, talent outputs, and the SQLite index all live here.

## Quick start

The journal ships as one self-contained tree. It needs no interpreter and no package manager of its own.

For release and local-build installation paths, see [INSTALL.md](INSTALL.md).

`install.sh` is already live at [solstone.app/install.sh](https://solstone.app/install.sh). Once a release is published, one command does the whole thing:

```bash
curl -fsSL https://solstone.app/install.sh | sh -s -- --version <version>
```

That fetches the archive from `updates.solstone.app`, verifies the digest, and installs. Today, with the files already on disk:

```bash
sh core/distribution/install.sh --archive solstone-journal-<version>-linux-x86_64.tar.gz \
              --sha256 solstone-journal-<version>-linux-x86_64.sha256 \
              --release solstone-journal-<version>-linux-x86_64.release
journal setup
```

On an Apple Silicon mac, the same command with `macos-arm64` in place of `linux-x86_64`. Debian and fedora can install the `.deb` or `.rpm` instead. One tree covers running the journal and talking to a journal that already runs elsewhere; there is no separate client download.

Not sure a computer is up to running the journal? After the tree is on PATH, `solstone check` gives a one-shot readiness verdict (gpu, memory, and disk) before you run setup.

Then open http://localhost:5015 in a browser. The first-run wizard sets up your identity and lets you choose a provider.

If you still have a pip, uv or pipx install of the old journal packages, [INSTALL.md](INSTALL.md#moving-from-a-pip-uv-or-pipx-install) is the migration. There is no CUDA package of the tree.

See [INSTALL.md](INSTALL.md) for prerequisites, the solstone app on your other devices, and troubleshooting. See [CONTRIBUTING.md](CONTRIBUTING.md) to develop on solstone from a source checkout.

## CLI

Use `solstone` for day-to-day journal access and `journal` for host operations.

```bash
solstone                    # Status overview and command list
journal supervisor         # Start the full stack (intake + processing + web)
journal transcribe <file>  # Transcribe an audio file
journal indexer            # Rebuild the search index
```

Run `solstone help` for the full command reference.

## Documentation

| Topic | Document |
|-------|----------|
| Installation and setup | [INSTALL.md](INSTALL.md) |
| Developing from source | [CONTRIBUTING.md](CONTRIBUTING.md) |
| Journal structure and data model | [core/payload/solstone/talent/journal/SKILL.md](core/payload/solstone/talent/journal/SKILL.md) |
| Intake pipeline | [docs/OBSERVE.md](docs/OBSERVE.md) |
| Processing and agents | [docs/THINK.md](docs/THINK.md) |
| Web interface | [docs/CONVEY.md](docs/CONVEY.md) |
| App development | [docs/APPS.md](docs/APPS.md) |
| Agent runtime | [docs/CORTEX.md](docs/CORTEX.md) |
| Message bus | [docs/CALLOSUM.md](docs/CALLOSUM.md) |
| AI provider configuration | [docs/PROVIDERS.md](docs/PROVIDERS.md) |
| What material reaches your AI provider | [DATA-FLOW.md](DATA-FLOW.md) |
| Troubleshooting | [docs/DOCTOR.md](docs/DOCTOR.md) |
| Project direction | [docs/ROADMAP.md](docs/ROADMAP.md) |

## Development

See [AGENTS.md](AGENTS.md) for development guidelines, coding standards, and testing instructions.

Use `make dev` to run the full stack against test fixtures, focused test targets
during development, and efficient `make ci` for routine validation. The
[Makefile](Makefile) defines both Rust gates; operators use `make ci-full` on the
exact final tree before merge or release.

## Feedback

Questions, feedback, or a bug: follow and tag [@solstone.app](https://bsky.app/profile/solstone.app) on Bluesky, open an issue at [github.com/solpbc/solstone-journal/issues](https://github.com/solpbc/solstone-journal/issues), or reach [support.solstone.app](https://support.solstone.app).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution terms.

## License

AGPL-3.0-only. See [LICENSE](LICENSE) for details.
Bundled third-party model notices: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
Maintained by [sol pbc](https://solpbc.org).
