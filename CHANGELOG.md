# Changelog

All notable changes to solstone (the Python package) will be documented in this file.

Format adapted from [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), aligned with `cmo/brand/changelog-voice.md`.

## [0.3.8] - 2026-05-22

### Added
- you can now run sol's on-screen analysis fully on your own Mac. on Apple Silicon with at least 16 GB of memory, "MLX (Local, Apple Silicon)" appears in Settings under Providers; choose it once, sol downloads a local model in the background, and from then on the part of sol that makes sense of your screen runs on your machine, with nothing sent to a cloud provider. it's opt-in and covers vision today; the rest of sol stays on whichever provider you've chosen.
- you can now power sol with Anthropic or OpenAI without installing anything extra. choose the provider in Settings and solstone sets it up for you, with no separate command-line tool to install first. running on a hosted Google key needs no extra setup either.
- `sol setup --clean-uninstall` removes the pieces setup added to your machine, behind a confirmation that lists exactly what it will remove. your journal is never touched.

### Changed
- the timeline view is rebuilt. it opens straight into your real journal, fits any window from a phone-width pane to a wide desktop, and every entry shows which AI produced it with a link to that day. when sol finishes summarizing a new day, the view updates on its own.
- long todo lists now load fast and stay readable: solstone shows the most recent items first with a "show more" control for the rest, instead of rendering everything at once.
- api keys in setup and Settings are now masked as you type, and the validate button tells you plainly whether the key connected or failed.
- on Linux, bringing an observer online no longer needs git or a build step on the host; observers now install straight from their published packages.

### Fixed
- video and audio in your journal that showed "format not supported" now play. some entries with video or audio hit this; it's resolved.
- on installs from PyPI, sol's meeting-screen analysis was coming back as freeform notes instead of the structured entries it was built to produce. the missing piece now ships with the package, so meeting frames return to their intended shape.
- transcription that gave up on a long, dense stretch of audio now retries and recovers, so days that previously failed to transcribe complete. this also recovered a backlog of past days that had errored.
- pages that occasionally didn't finish loading now load cleanly.
- on some machines the background service could stop overnight and not restart; it now restarts as intended.
- pairing a phone by QR code now works in Safari on iPhone and Mac, where the code could previously render too small to scan.
- internal stability improvements, plus quieter local logs.

## [0.3.6] - 2026-05-18

### Changed
- solstone now uses each provider's current models, and the structured results sol asks providers for are validated the same way across every provider, including the backup one. this makes the AI features more reliable, with no change to how you use solstone.

### Fixed
- in some cases what sol wrote to your journal from a screen could be off. a frame with little on it could pick up names from your own contacts as if they'd been on screen, and an occasional runaway from the model could write a long block of repeated text into an entry. both are now caught before anything is written, so your journal reflects what was actually there.
- when sol fell back to a backup AI provider for a task that involved an image, the image could be left out of the request, so the result was a confident guess instead of something grounded in what was on screen. images are now sent correctly on every provider, and structured results from the backup provider are read correctly.
- upgrading solstone over an existing install now works cleanly. before, an upgrade could stop partway: setup could wrongly report that port 5015 was in use when it was solstone's own running service, and re-registering this machine's observer could fail as "already exists." if an upgrade left you stuck, this resolves it.

## [0.3.5] - 2026-05-17

### Added
- a new data-flow page explains, in plain language, what solstone sends to your chosen AI provider and what never leaves your machine. it covers local-first processing, that each task is scoped (not your whole journal), that the keys and the account are yours, and the things sol pbc is bound never to do with your data. it's linked from setup, the install guide, and the readme so you can read it before you connect a provider.
- the install guide now has a section on how to power sol: starting with a hosted provider key is recommended, running fully local is a real supported goal but not yet the default daily experience, with a heads-up on the hardware that takes. setup and the api-key settings now also tell you, per provider, to use a developer api key from the provider's console rather than your consumer chat login, with the right console link for each.

### Changed
- in-app support and feedback now point to support.solstone.app, and that's the default in support settings for new journals. if your settings still point at the old support address, nothing breaks and you can leave it as is. setup, the install guide, and the readme now also lead with following and tagging @solstone.app on Bluesky for feedback, then GitHub issues, then the support site.

## [0.3.4] - 2026-05-16

### Added
- a fresh journal now opens with a useful set of starred apps in the nav rail instead of a blank one. if you've already arranged your own starred apps, your choices are left exactly as they are.

### Changed
- the deprecated `precision` setting for parakeet transcription has been removed. `quantization` (auto, fp32, or int8) is the setting to use. if your journal config still carries the old `precision` line it's now simply ignored, with no change to how transcription runs.

### Fixed
- browsing back from the all-facets entity edit view now returns you to the entity you were looking at, in the same facet. before, back could land you on a different view.
- the bundled `transcripts read` documentation now shows the correct options. the previous example listed the wrong units for `--start` and `--length`, so following it as written would have failed.

## [0.3.3] - 2026-05-16

### Added
- a validate button now sits next to the gemini api key on the setup page, so you can confirm the key works before finalizing.

### Changed
- the setup page is reworked: cleaner typography, retention preferences as three explicit choices (always keep, keep for a set number of days, don't retain), enter-to-submit from any field, and your journal version and path surfaced up top.
- a fresh `sol setup` now installs the solstone bundle into all three coding-agent configs (claude, codex, gemini) at once, and lands the per-talent skill files in your journal so sol's sub-agents can find them.

### Fixed
- the setup page works end-to-end on a fresh install. earlier builds had a silent javascript bug that left the validate button, retention radios, and finalize submit unresponsive.
- on macos, your local timezone now resolves correctly on first setup. earlier installs could land in utc because the resolver missed where macos stores zone data.
