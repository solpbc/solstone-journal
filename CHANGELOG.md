# Changelog

All notable changes to solstone will be documented in this file.

Format adapted from [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [2.0.0] - 2026-08-18

### Changed

- the journal-access command is now `solstone` only. `sol` as a command name is gone, with no alias: a script, cron job, or muscle memory that still types `sol` needs to type `solstone` instead. `journal` is unchanged: it is still the journal's own command, separate from day-to-day journal access. `solstone call <app> <verb>` is the form that used to be `sol call`. the solstone app on your devices is unchanged by this rename.
- chat is gone. you have a working chat today. this upgrade takes it away. the chat bar in your journal, the chat page, and `solstone chat` are gone. on iphone, the ask bar on the day home is gone too, and nothing takes its place. that screen is your day.
- chats you already have in your journal stay on your disk, untouched. they are no longer shown, and they no longer come up in search.
- solstone is a personal memory platform. to ask questions of your journal, use your own agent, or the command line.
- filing support through the chat bar is gone. compose, review, and send now happen on the support page. the review card shows every field and every diagnostic value before anything leaves your machine.
- chat requests to your phone are paused, not gone.
- the timeline view is gone. your journal no longer picks a headline event out of every five-minute slice, or picks the few events it judged most important to stand for a day, a month, or a year. the timeline entries it already wrote stay on your disk, untouched, and nothing reads them. a link that used to open a day in the timeline opens it in the transcripts view now.

## [1.0.22] - 2026-08-01

### Changed

- turning on confidential processing finishes on its own now. once you finish turning it on in your browser, your journal runs its check and shows "confidential processing is ready" when it passes, instead of stopping short until you press check again. that button is still there, and a setup that needs repair, or one still in progress, is left alone.
- your journal no longer gets you a Gemini key. that option is gone from the thinking app and from the command line. the scout program itself is still running. a Gemini key already saved in your journal keeps working, untouched, with nothing to re-enter and nothing to reconfigure. choosing Gemini and adding your own Google AI Studio key is unchanged: same field, same models, same check that the key works.
- pairing your browser with your journal, and sending pages to it that way, is gone. sol in the browser is on hold for now. pages already in your journal stay exactly as they are, searchable and readable.
- on an Intel mac, the journal now says plainly that Apple Silicon is what it needs on a mac, and points you at where it does run: an Apple Silicon mac, or supported linux.

### Fixed

- your journal turns away malformed device authorization it used to let through. an entry in its list of authorized devices could be damaged in a way that still read as a valid certificate. separately, a device whose pairing was stored as empty could skip the check that ties it to that list. both are refused now, before anything gets in. neither shape is one your journal writes, so a device you paired the ordinary way authorizes as it always has. if a device does stop authorizing after this update, pair it again.

## [1.0.21] - 2026-08-01

### Changed

- adding a device by hand is gone. the page in your journal that lists your devices no longer offers it, and the request behind it now refuses. pairing is how a device joins your journal.
- `sol export --to <url> --key <key>` is retired. it refuses now and points at the paired form instead. `sol export --to <label>`, sending to a journal you have already paired with, is unchanged and works as it always has.
- the part of your journal that carries the private network is now native Rust in place of Python. your paired devices reach your journal exactly as before, over the same private network, with nothing to re-pair and nothing to set up. it runs as a single program that starts and stops with your journal, with no Python in between.
- two devices with the same name are told apart now. a second device calling itself "iPhone" is listed as "iPhone (2)" rather than reading the same as the first. on a journal that already holds two devices sharing a name, one of them takes its number on its own the first time the journal starts after this upgrade, so a device whose name reads differently afterwards is that, not a fault. the name you give a device is the name that shows, and the name a device proposed for itself at pairing is noted separately.

### Fixed

- unpairing a device by name could take away the wrong device's access. when two of your devices shared a name, unpairing by that name acted on whichever one it found first. separately, a device that was reinstalled and paired again would revoke the older entry with the same name on its own, whether or not it was the same device. unpairing by name now only proceeds when the name points at exactly one device, and when it does not, it lists each candidate with the date it was paired and the exact command for the one you mean. pairing again revokes nothing, and an entry you no longer want stays until you remove it.
- speaker analysis works on your audio again. on the last two releases every clip was turned away at that step as invalid, so nothing was analyzed for who was speaking.
- a local model that has finished starting is left running. a model server that had loaded and was answering could be stopped about a minute later, because the check for whether there was room to start one was re-run against memory the model itself was now holding. it would start again and be stopped again, so a day's work could never finish and a backlog could sit still. that check now only decides whether to start a model, never whether to stop one that is already serving.
- an import you open while it is still working now says it is running. the import history listed it correctly, but opening that same import showed it as failed until it finished — on every file type, and most noticeable on longer imports. the page now also keeps up on its own while the import runs, instead of waiting for a reload. reported and fixed by ha-ye.

## [1.0.20] - 2026-07-30

### Added

- `journal reprocess <start> --through <end> --from-scratch --yes` redoes a range of days rather than one at a time. days queue oldest first and run one at a time, so a long range can take hours and the current day waits until it finishes. `journal top` and `journal health` show where it is.

### Changed

- importing from Granola is gone. Granola removed the local file access this import read from, and every remaining route to that data needs a paid Granola plan and a network call, which this import never made. a scheduled Granola sync goes with it, so nothing is left retrying every hour. Granola transcripts already in your journal stay exactly as they are, searchable and readable.

### Fixed

- an upload your journal turns away now leaves a trace. something can arrive carrying a key your journal does not know, or one that has since been withdrawn, and it is refused. until now that left no sign anywhere: no way to see it had happened, or how often. a refusal is now noted while it is going on, and again once it stops, with how many were turned away.
- what sol takes in on a paired device reaches your journal byte-for-byte. a read over the link could add two stray bytes to the front of a file while still reporting that the transfer finished.
- a day that stalled on a local model moves again. work could be handed to a model that was still loading and fail there, and in chat a reply that was still being written could be ended with an error while the answer arrived moments later. queued work now waits for the models it needs to finish starting instead of waiting on a timer, and a reply still being written is no longer read as a stopped one. when a day is held back for a retry, process now says so instead of reporting it queued.
- a model that declines a piece of work no longer has its refusal kept as the answer. text along the lines of "I cannot view images" could be written into your journal as a screen description, or come back in chat as a reply. that now ends with a stated reason and nothing written, and a model that keeps declining the same frame is no longer asked over and over.
- an import started while another one is running now waits its turn. it used to report that it started and then never run, and the file it came from could not be started again afterwards. an import that failed or never ran can now be started again, and a start that cannot be handed off to run says so instead of reporting success.

## [1.0.19] - 2026-07-29

### Added

- the entities overview in your journal now shows what each entity is, read from that entity's own record and never guessed from its name. an entity with no type of its own still appears in the list with everything else about it unchanged.

### Changed

- speaker analysis now uses the native helper only. if the helper is missing, damaged, or returns invalid data, journal startup and transcription fail loudly instead of falling back to the old python speaker path.
- new transcript headers name `solstone-core-speakers-analyze-v1` as the speaker-analysis producer whenever the helper actually ran, including when it declines speaker labels. transcripts with no speech-bearing statements still omit the field because no speaker-analysis producer touched them.
- long audio no longer quietly changes engines. with confidential processing turned on, audio longer than the confidential service accepts used to be handed to the built-in local engine instead. it now stops with a stated reason and the audio is kept as it is, so nothing gets processed somewhere you didn't choose.

### Fixed

- what sol takes in on your paired devices keeps reaching your journal. an unsteady connection could leave a transfer stalled and holding one of the journal's slots for incoming work indefinitely. enough of those and nothing arrived at all, with no error, and with pairing, the connection, and the device's last-seen time all still reading healthy until the journal was restarted. stalled transfers are now bounded and cleared, everyday arrivals have room reserved for them so long streams can't take every slot, and a stall that does happen is logged with how long it waited.
- a day sol is thinking through no longer stalls on unusual text. a stray half of an emoji, or another unpaired character, could stop the request while it was still being built on your machine, and the failure was reported against the endpoint, which never received it. that text is repaired before the request is built now, and text that was already valid is unchanged.

## [1.0.18] - 2026-07-28

### Added

- `sol link join` and `sol link serve` are now native commands, like `sol` and `solstone` before them. same commands, same pairing, and no python on any path they own.

### Changed

- `journal link` is gone. listing paired devices, checking a device's status, unpairing one, and seeing what's authorized now live in the network app in your journal, with no command-line equivalent. anything built on `journal link` needs the network app instead.
- the base install is thinner. `pip install solstone` no longer brings along the compiled cryptography and websocket libraries only the journal needs; they install with `solstone-journal` and `solstone-journal-cuda`, so a journal install is unchanged and pairing works the same way. anything that relied on those libraries being present in a base install needs a journal package now.

### Fixed

- installing the Apple Silicon `solstone-core` wheel no longer leaves its signing facts as a stray file in site-packages. the signing evidence stays in the release rail instead of the installed package.
- sol thinks through your days more reliably. a run through a local model, confidential processing, or an endpoint you bring could be refused for being too long: the length sol measured before sending came out under what the model itself counted, so sol now leaves room for the difference. a run could also stall on sol retrying a command in a form your journal doesn't accept; sol now gets the working form back instead.
- the voice evidence in your journal stays with the voice it came from. when your journal grouped a conversation's voices again and came out with a different number of them, statements could shift onto a different voice, either one it was still learning or a person you had already named. a voice now carries the statements it was built from, so this can no longer happen. entries from before this release that carry none are skipped rather than guessed at, and a maintenance pass recovers the ones it can.
- a day's record of sol's thinking is no longer cleared before its time. those records were aged by the date in their filename while the runs inside them could be from the last day, so a recent one could be removed early. a record is now kept unless everything in it is genuinely past the cutoff, and one that can't be read is kept rather than discarded.
- the apps in your journal read and behave better. faint text in the health logs, in empty-state messages, and on the body app's main button now has enough contrast to read; the charts in stats no longer overlap their own labels or push the page sideways on a phone; and tokens shows the day you're looking at, where an earlier day used to show today's numbers. Tab stays inside an open dialog instead of jumping out to the browser, the network app's pair dialog closes when you click outside it, and closing a dialog no longer leaves part of the page behind it unresponsive.

## [1.0.17] - 2026-07-26

### Fixed

- setting up your journal now finishes cleanly. two of its own steps were failing on every run, so setup reported a problem at the end even when your journal was fine, and on a Mac a brand new install could sit waiting at the last step and never finish.

## [1.0.16] - 2026-07-25

### Fixed

- pairing a device with your journal over your own network now completes. attempts your journal had already authorized were reaching the device as a failure, often reading there as a pairing code that had expired. every attempt that failed this way still left an authorized device behind, so open the network app and remove any unnamed devices you don't recognize.
- `sol link join` now reads a pasted pair link in full before it connects to anything. an address in the link that sits outside your own network refuses the whole link rather than being skipped, the identity of the journal you're joining is confirmed from the connection itself before the request goes out, and each run makes exactly one pairing request. a run that fails partway no longer leaves a half-written credentials folder behind.
- if you use Google as your AI provider, sol can think about your days. Google was refusing nearly every request sol sent about a day, so your days were taken into your journal and never thought through, while your journal's health still reported that sol could think.
- home now tells you when a day in your journal is stuck, or when it can't tell whether your journal is caught up. before, home could still show as healthy in both cases.
- on a Mac, doctor no longer warns that your `sol` and `journal` commands were installed by something else and asks you to run `journal setup`. every install done by the macOS app got that warning, and running setup would have replaced a working launcher.

## [1.0.15] - 2026-07-25

### Fixed

- installing with `uv tool install solstone` now puts the `sol` and `solstone` commands on your path. the 1.0.14 install could finish without them.

### Changed

- the main package now owns the public `sol` and `solstone` commands, and the native package owns the compiled program they run.

## [1.0.14] - 2026-07-24

### Added

- `sol` and `solstone` are now native commands on supported Linux and Apple Silicon Mac systems, with the same Python compatibility commands still available while the final porting work finishes.
- unsupported platforms now stop during installation with a clear message instead of installing without a working `sol`.

### Changed

- the native command package now owns `sol`, `solstone`, and `solstone-core`, so installs have one deterministic owner for each executable.

## [1.0.13] - 2026-07-24

### Added

- the `sol` command can now turn confidential processing on, check its verification again, turn it off, and show where setup stands.

### Changed

- thinking through a local model, confidential processing, or an endpoint you bring now fits each request to the model's real working limit. longer prompts and image-heavy work retry once at a tighter limit instead of repeating the same failure.

### Fixed

- confidential processing now renews its verification before it expires, so a journal can stay connected across long unattended runs. if verification cannot renew, the connection stays closed before any request is sent and recovers on its own after verification succeeds again.
- a routine confidential-processing status check now shows the operation phase without repeating its one-time setup link.
- damaged or empty screen video now reaches a finished error state instead of leaving the whole day waiting for analysis forever.

## [1.0.12] - 2026-07-22

### Added

- every release now publishes signed evidence of exactly what shipped at transparency.solstone.app. anyone can check a release against it independently.

### Changed

- one version number for the journal everywhere now. the pip-installed journal and the native journal apps used to count releases separately; from this release they share the same version, which is why the number jumps. same journal, nothing was skipped.
- on supported machines, the journal's index is now updated by new native code. what the index holds is unchanged, and a setting can put the previous indexer back at any time.

### Fixed

- a journal with leftover confidential processing settings, but with the built-in local model doing its thinking, could stop thinking entirely. sol now goes by where thinking actually runs, not by what's left in settings, so the built-in model keeps working. while confidential processing is on, switching providers now requires turning it off first, so a settings change can't leave the journal in that state again.
- the same leftover settings could quietly stall transcription while everything else looked fine, leaving new audio waiting. transcription now falls back to local speech-to-text in that state. the separate safeguard that decides whether audio may leave your machine is unchanged.

## [0.9.1] - 2026-07-22

### Added

- recurring voices now open in one who-is-this sheet from home, speakers, or a transcript. it shows the evidence, searches known people before offering a new person, keeps similar names separate only after confirmation, routes your own voice to its teaching path, and lets you preview, apply, or undo corrections across matching statements. speaker identification is also more cautious when voices sound alike.
- the journal now names which brain produced each new image description and each run of screen descriptions.

### Changed

- thinking status now agrees everywhere it appears. background checks, blank replies, and model readiness all resolve to one state with one next action.
- sol now suggests exact Gemini models instead of moving aliases and keeps confidential processing active while saving a model for later. confidential processing sets up verification on first use and names setup or integrity problems, while a failed local-model replacement leaves the working model available.

### Fixed

- image imports now reach each brain in a format it understands, including webp with local models. markdown with accented or non-latin text keeps the same size and grouping rules as other text, and an interrupted index update no longer leaves a partial result.
- links inside the journal on ios now open reliably instead of stalling. `journal doctor` now finds background launchers that keep reopening the legacy mac app, and a full index rescan explains why it found no new relationships and how to rebuild them.

## [0.9.0] - 2026-07-19

### Added

- the calendar day grid now reaches entities and news. pick a day on an entity to narrow its page to that day, and browse news one day at a time.

### Changed

- setting up a local AI model no longer loses your settings when two changes save at the same time, and the install status now reflects what's actually ready instead of showing a model as ready before it can run.
- speaker naming is more accurate. sol is less likely to mistake another person's voice for yours, and busy conversations with many similar-sounding voices now settle to the true set of speakers for you to review and confirm.
- on Linux, local speech-to-text now uses much less memory, so more graphics cards can run it on the GPU instead of the CPU. transcription quality is unchanged.

### Fixed

- when you send anonymous feedback, it now uses a generated handle instead of your device's name, no longer attaches diagnostic details automatically, and scrubs secrets from any error text before it's included.
- if a local AI model stops responding, your settings now show a repair action that gets it working again in place, without restarting your whole journal.

## [0.8.9] - 2026-07-17

### Added

- journal installs now carry a native `solstone-core` helper on Linux x86_64, Linux aarch64, and Apple Silicon macOS. the Linux wheels are static manylinux2014 builds, and startup now checks that the helper beside your active Python belongs to the installed `solstone-core` package before the supervisor touches the journal. the helper is installed and checked now; the journal does not use it for work yet.
- your journal can now keep older media in your backup instead of on disk. it's off until you turn it on: you set a media budget and a free-space floor, and when the original screen and audio files pass them, the oldest fully-processed media your backup has confirmed moves out, oldest first, with the local copy removed only after that confirmation. transcripts and everything else your journal holds stay where they are, and any offloaded day restores on demand. once media has moved, your backup holds the only copy of it: turning offload on walks through your recovery key first, offloaded media is pinned so your backup's normal rotation can't age it out, and a weekly read-back check samples the backup itself. if a backup lapses or a check fails, offload halts with nothing deleted and says so on the backup page, and turning your backup off now tells you what it holds the only copy of and offers to restore everything first.
- timeline, search, speakers, and reflections now show your days as a calendar grid. timeline reaches back through your whole journal instead of stopping twelve months ago, search lets you drag a date range right on the grid, speakers shows which days still need names, and reflections shows which weeks have one.

### Changed

- analysis that stops partway now picks itself back up. your journal sizes its analysis work to what your model can actually serve, treats a busy endpoint as busy rather than unreachable, and re-runs a failed day on the next daily pass, paying only for the frames that failed and keeping the ones that already worked. before, the densest screen days could sit unanalyzed indefinitely, and a day that failed stayed failed until you re-ran it by hand. after three tries your journal stops and the day completes with the gap named in its health summary rather than holding the day open.

### Fixed

- a segment whose analysis had failed could still be marked finished, and the check that removes raw media once it's processed trusted that mark. the mark is now written after every stage completes, and any unresolved error holds the raw media in place. your journal keeps raw media unless you've asked for it to be removed after processing, so that setting is who this reached. it's the same mark offload reads before any media moves.
- changing your retention settings from the backup page now saves. if you tried and got an error, this resolves it.
- naming yourself as a speaker now works on a fresh journal. before your voice had been identified, tagging a sentence as you failed on every sentence, which left no way to seed it. when detection can't find your voice, the speakers page and the `sol` command now name the next step rather than stopping.
- `journal doctor` now detects when the macOS app and the legacy background service both target one journal, and points to the single service removal step before other repair actions.

## [0.8.8] - 2026-07-16

### Added

- your journal now keeps pages from the solstone browser extension: what you read in the browser shows up in the timeline and transcripts, site by site, and sol's thinking works from pages along with everything else.

### Changed

- journal audio transcription now uses only the local Parakeet paths or the confidential lane; legacy Rev.ai/Gemini STT, default transcript enrichment, noisy-audio cloud upgrade, and the `google-genai` dependency were removed.
- machines below the local STT memory floor now surface a local/confidential setup requirement instead of falling back to hosted Google transcription.
- thinking now has one active brain for every task: bundled local by default, a personal OpenAI, Anthropic, or Google AI Studio key, or an owner-supplied OpenAI-compatible endpoint. legacy split lanes, tier and talent routing, Vertex support, and duplicate cloud-provider adapters have been removed.

### Fixed

- a key set for an owner-supplied thinking endpoint could appear in error details when a request failed. keys are now removed from every error, event, and trace.
- the settings page's data included whether cloud thinking keys were set. it no longer does; thinking is the only place that holds provider configuration.
- pointing thinking at your own compatible endpoint is more dependable: requests no longer carry local-only fields some endpoints rejected, and an endpoint that can't be reached now says so quickly instead of timing out.
- the bundled local model now downloads at full speed on first setup; some installs saw it crawl at a fraction of their connection's speed.

## [0.8.7] - 2026-07-15

### Added

- older journals with repeated copies of the same segment can now preview and safely remove only proven duplicates, including copies filed under different start times.

### Changed

- pdf imports now handle each page separately, keep extracted text distinct from model-assisted text, and preserve a page image anywhere a model helps.
- dated journal views now use one in-page date control, with day, month, and year zoom plus week-by-week movement for reflections.

### Fixed

- when sol sends the same moment again, your journal now reuses what it already has instead of creating extra copies.
- text, markdown, and document imports that produce nothing now say so clearly and can be retried without a false "already imported" result.
- private-network errors now stay contained, so one failed request no longer drops the whole link or forces repeat pairing.

## [0.8.6] - 2026-07-14

### Fixed

- on a fresh install, checking your thinking providers now reaches every provider instead of stopping before the checks begin.

## [0.8.5] - 2026-07-14

### Added

- confidential processing can now be enabled from the thinking app and includes a separate audio transcription switch. when audio transcription is on, that step uses the same verified confidential connection as thinking; the rest of audio processing stays local.
- you can now bring Apple Health exports and Oura into your journal, then explore sleep, activity, heart, recovery, and other signals by day or across trends. your raw-retention choice is enforced when the import is saved, saved health material has owner-only file access, and its summary cards stay out of sol's thinking.
- the home and entities views now show how people and organizations connect. identity changes keep a history, merges can be undone, and an ambiguous name waits for your choice before the journal changes anything.
- when you add a Google, Anthropic, or OpenAI key, the thinking app checks it before anything is saved. then you can choose from three model options or name a specific model, which is checked before it becomes sol's active brain.

### Changed

- sol now uses one brain chosen in the thinking app for each lane and no longer ranks or switches engines behind the scenes. older tier, routing, and fallback settings remain untouched but no longer steer model choice. if you bring a paid vendor key, some frequent work may cost more because it now uses the model you chose.

### Fixed

- long hosted backups and restores no longer lose access partway through. each job now uses narrowly scoped access that lasts for the operation, while older snapshots remain append-only.

## [0.8.4] - 2026-07-12

### Added

- when solstone suggests merging duplicate people or companies in your journal, you can now review several of those suggestions at once. select a batch and accept or dismiss them together instead of one by one; if some can't be merged, the rest still go through and the ones that need attention stay listed.

### Changed

- sol's chat replies now display formatting properly, so lists, emphasis, and code render instead of showing as raw text, and long code blocks stay contained inside the reply rather than stretching it wider. when you ask sol about your own life or history, it also now checks your journal by default instead of answering from general knowledge, while everyday conversation like greetings and follow-ups still answers directly.
- setting up local thinking now shows live progress while the model downloads and installs, picks up where it left off if you reload the page, and shows a clear error with a retry if an install doesn't finish.

### Fixed

- the words in your transcripts are never meant to appear in solstone's internal error reports. we closed a gap where a failed transcription could put a short fragment of that text into one, and made the guarantee structural: those reports now carry only the kind of error, never any of your content.
- local thinking and transcription handle their limits more honestly now. when a request is too long for the local model or your machine is busy, sol says so in plain words instead of showing a raw code; when local transcription can't finish a clip, sol keeps the audio and retries next time instead of quietly treating it as finished; and truncated local runs no longer leave partial results in your journal.

## [0.8.3] - 2026-07-10

### Added

- the journal now builds a rebuildable relationship layer from evidence already in your journal. it connects people, projects, decisions, and documents without invented links.

### Changed

- setup and thinking settings now separate the three ways for sol to think: local on this machine, confidential processing, or an engine you bring yourself. on capable machines, local setup starts in the background; when no engine is chosen, sol says so plainly instead of choosing one silently.

### Fixed

- local thinking on your own machine is steadier under busy stretches and backlogs. sol now keeps repeat loops from taking over a run, waits out brief overload, and avoids starting more work than the local engine can handle.
- on machines where local model work shares system memory, sol now waits for memory to free up before starting heavy work, instead of launching into out-of-memory failures.
- speaker review can get through the first speaker setup on fresh or migrated journals. it finds a starting voice sample quickly, plays audio from mac journals, and offers your own name when the journal is new.

## [0.8.2] - 2026-07-07

### Added

- sol now notices calendar and scheduling moments and files them as their own category in your journal, so appointments and plans are easier to find.

### Fixed

- the screen and combined-transcript tabs no longer go blank when a moment comes through in an unexpected shape. if you'd seen either tab empty out, this resolves it.
- the home page no longer shows the same thing to do twice across its "needs you" and "needs attention" lists.
- adding an alternate name to a person or company no longer trips a false "name already in use" warning against that same person or company. real conflicts between different ones still flag.
- local thinking on your own machine is more reliable. sol now uses the right runtime for your hardware instead of quietly falling back to cpu, and local requests that use time-like patterns no longer fail before the model starts.
- on linux, sol check no longer reports "no usable gpu" when the gpu is present but the current user lacks permission to reach it. it now flags this as a permission problem you can fix instead of a wrong "no gpu" verdict.

## [0.8.1] - 2026-07-07

### Added

- `sol check` (also `journal check`) gives a one-shot readiness verdict for whether this computer can run the journal with the bundled local models — a zero-install pre-flight you can run with `uvx solstone check`, human-readable by default or `--json` for agents.

### Fixed

- sol-only installs now render provider readiness in `sol call health summary` without importing journal-only model setup code.

## [0.8.0] - 2026-07-07

### Added

- journal search now starts with exact matches for agent calls and returns an honest zero when nothing matches, so agents can broaden their own query instead of getting silent near-misses.
- sol can now add local media, social, and ambient-sound hints to material already kept in your journal, using local model installs that stay on your machine.
- the journal can prepare and verify local vision and audio helper models from settings, including host-fit checks before a download starts.

### Changed

- journal screens now stay in one app frame, so moving between home, health, settings, search, transcripts, and the other screens is faster and steadier.
- media and social classification now use one shared prompt, so the same moment is described more consistently across install paths.

### Fixed

- search totals and segment details are now de-duplicated, so agent and journal search results no longer over-count the same stretch of the day.
- settings pages no longer flash or blank when switching views in the new app frame.

## [0.7.0] - 2026-07-05

### Added

- `sol doctor` and `journal doctor` now check that the installed packages fit together, and name the exact fix when they don't. the checks read only what's installed locally, nothing goes over the network.
- the `journal` command now refuses to start when an upgrade left packages out of step, and prints the fix command instead of running with mismatched pieces.

### Changed

- the journal is now its own package: install `solstone-journal` (or `solstone-journal-cuda` for NVIDIA GPUs) and sol comes included. bare `solstone` still installs just sol.
- the old install spellings (`solstone[journal]`, `solstone[journal-cuda]`, and `solstone-journal-host`) are retired. they now stop at install time with a message pointing at the new packages, instead of quietly producing an install missing the `journal` command.
- a sol-only install is much smaller: the speech models that used to ship inside every install now come with the journal, so a plain `solstone` install (and `uvx solstone` one-shots) drops from about 34 MB to about 3 MB, and journal upgrades no longer re-download models that haven't changed.

### migrating from an earlier version

if you already have solstone installed, run the one line that matches how you installed it:

```
pip uninstall solstone-journal-host && pip install solstone-journal
pipx uninstall solstone && pipx install solstone-journal && pipx install solstone
uv tool uninstall solstone && uv tool install solstone-journal && uv tool install solstone
```

## [0.6.24] - 2026-07-04

### Fixed
- if screen processing fell a few days behind, sol now catches up instead of stalling. it used to chip away at a backlog in one short nightly window, one piece at a time, so a pile could sit for days; sol now works through more at once and keeps going until the backlog clears.
- a stretch of the day still being processed is no longer mistaken for an empty one. sol used to sometimes mark that stretch done too soon, so what was on your screen never made it into the day's summary; it now waits and comes back, so the summary in your journal reflects the full day.

## [0.6.23] - 2026-07-04

### Added
- a browser observer can now sync to your journal even when the journal runs on a different machine. it reaches your home over the private relay and seals what it sends end to end, so the relay only moves the bytes and can't read them.

### Changed
- the journal's own screens and the customer-facing docs now follow the sol naming model: sol lives on your devices and keeps everything in your journal.

### Fixed
- sol reports its own health and a day's processing truthfully. status and health views now show a real state instead of an optimistic guess, a day where every image read failed is marked done-until-you-redo instead of looking stuck, and work that already retried and finished is no longer counted as a problem.
- your journal holds onto your data more safely. a single corrupt reading no longer aborts the whole day, imported data is checked against its fingerprint before it's trusted, a segment copy either finishes or leaves nothing half-written, backups keep the health record they used to drop, and older media is only cleared once sol can confirm the results derived from it are safely stored.
- hardened the journal's local connections. the local web surface now turns away requests from unexpected sites, audio and segment files are only served from inside your journal, and searching with an apostrophe no longer errors out.

## [0.6.22] - 2026-07-03

### Added
- you can now pair a device to your journal over a Tailscale network, not only a shared wi-fi. if your phone and your journal are on the same Tailscale network but different wi-fi, that direct connection now works for pairing.

### Fixed
- sol's summary of a day's processing now reflects what actually happened. the 'yesterday's processing' view used to flag problems on nearly every normal day, counting work that had already retried and finished, or that belonged to a different day, and cutting long lists short. it now shows only what genuinely didn't finish for that day, grouped by name with an honest count.
- sol gets through a full day's processing more reliably. scheduled work that didn't run at its set time is now retried instead of waiting out a full cycle, the switch to a new day at midnight no longer gets stuck, and if one step runs into trouble sol keeps going with the rest instead of stalling. work that sol used to occasionally lose track of now either finishes or is recorded as a clear failure.

## [0.6.21] - 2026-07-02

### Changed
- on linux, sol now uses a single local engine to turn your speech into text. it runs entirely on your own machine, and your audio never leaves it.
- the first time that engine runs, it downloads the model files it needs to work, the same as the previous engine did. that download only brings files in; nothing from your journal goes out.

## [0.6.20] - 2026-07-01

### Added
- on linux, you can now opt into an alternative local transcription engine. the default engine is unchanged.

### Changed
- sol now does less repeated work on the video and on-screen activity it takes in. it skips near-identical moments while keeping the ones where something on screen changed, so nothing that changed gets missed.

### Fixed
- catching up on past days is steadier. sol re-does only the parts of a day that didn't finish rather than walking the whole day from the start, and it skips audio it has already turned into text.
- if one audio file in your day couldn't be read, it used to be able to stop the rest of the day from being transcribed. now sol sets that one file aside and keeps going, so the rest of your day still becomes text in your journal.

## [0.6.19] - 2026-07-01

### Fixed
- a stretch of your day with nothing for sol to think about, like a quiet passage with no speech or one where your phone only had your location, used to keep the day from finishing. sol would keep re-trying that stretch and never mark the day done. now sol recognizes there's nothing there to read, marks that stretch done, and moves on, so the day completes.
- if one step sol runs on a stretch of your day kept failing, it could hold the whole day open, re-attempted each time solstone caught up and never finishing. now sol sets a step aside after it fails enough times in a row, so the day completes, and leaves it there to try again later. your transcript and media stay in your journal the whole time.
- asking sol to re-process part of a day is steadier now. it used to be able to get wedged partway and not restart, and now it recovers. and if an earlier version already left days stuck this way, running `journal backfill-processing-records` clears them so they can finish, previewing the change before you apply it.

## [0.6.18] - 2026-06-30

### Added
- you can now pair a device to your journal over your private network from the command line, even when that device isn't on the same local network as your home. your home opens a brief pairing window directly to do it, instead of relying on a short code that kept rotating, so it's more reliable and there's no shared code sitting anywhere in between. if the window isn't ready yet, pairing waits for it and tells you plainly when it's unavailable, rather than failing with a confusing error.

### Fixed
- your private network's status now stays honest while it's running. it used to be able to read as connected while the link that reaches your journal from afar was actually failing. now it tells those apart, recovers on its own when the link goes stale, and never puts access tokens or connection secrets into its status or logs.
- a local command that talks to your journal no longer waits forever if the journal stops responding. it now stops after a set wait and tells you "the journal didn't answer in time," instead of hanging.
- status about finishing up your days is honest now too. when sol re-processes part of a day or re-analyzes a transcript, a step that had only started, or hadn't been verified, used to be able to read as done. your health could even read as well while it hadn't. anything that hasn't truly finished and produced a result now shows as still running or as a clear error, and a day that didn't finish re-processing no longer reads as fully caught up.
- catching up on your days is steadier when something goes wrong. if sol gets stuck finishing one day, solstone now keeps working through the rest of your days instead of letting that one hold everything up, and a single stalled step can no longer tie up sol's daily work.

## [0.6.17] - 2026-06-29

### Added
- you can now begin pairing over your private network without waiting for the whole pairing to finish first. start it and carry on while it completes.

### Changed
- your home screen now shows your solstone's health as a single honest verdict. it rolls every signal into one line, always reflects the most serious thing happening, and never reads "everything's working" while something is failing or can't be checked. anything it flags is now a link you can follow to learn more or fix it, and it will tell you if sol has no working way to think. day-to-day activity counts moved off the glance so it stays focused on what needs your attention.

### Fixed
- deleting a location source now also removes location data that was bundled inside mixed mobile observations, where it used to be left behind.
- if an observer was running but its observations were silently never reaching your journal, solstone used to show it as fine, unknown, or stale. that state now shows up clearly wherever you check on solstone, in red, with what went wrong and how to recover.
- observations from a terminal or screen-only observer, which have no separate media to send, were being turned away and set aside. they're now accepted into your journal.
- when turning on your private network runs into a problem, you now get the specific reason and what to do about it, instead of one generic error message.

## [0.6.16] - 2026-06-28

### Added
- setting up solstone now gives your journal its own identity. you'll see your journal's mark, a small visual of two icons and two words derived from your journal's own key, and you can regenerate it until you like the one you get, then lock it in. your journal id comes from that same key, so it's unique to your journal and the same wherever it appears. after setup, your mark and journal id also stay visible where your devices connect to your journal.

### Changed
- if you run solstone fully on your own device, sol's local work, like summarizing your day and your weekly reflection, now stays on your device as intended even when no cloud key is set up. before, some of it could stop with a setup error. this keeps more of sol's work on your device.
- re-registering an observer after a restart or reinstall no longer creates a duplicate entry; it reuses the one already there. a new `observer reconcile` command also collapses any duplicate observer entries you already have.

### Fixed
- re-pairing a device after a reinstall or restore used to leave its previous credential active, so old credentials could accumulate over time. re-pairing now retires the credential it replaces, leaving one current credential per device.
- a chat reply that took too long to finish could later appear as the answer to a different message. that no longer happens. a reply that times out is now closed out cleanly and can't carry into your next message.
- when sol prepared a message to solstone support for you to review, it could be described as a failure to reach support even though the draft was ready. the wording now matches what happened: your draft is prepared for you to review and send.

## [0.6.15] - 2026-06-26

### Added
- you can now give any facet its own icon. open a facet's appearance settings, search the icon set, see each option in your facet's color, and apply the one you want. your emoji stays the default, so you can switch back to it anytime.

### Changed
- the menu in your journal's web app and your facets now show clean, consistent icons. each facet's icon comes from its emoji by default; the icons are simply a crisper way to render it.
- for new owners, the welcome now sets a more accurate expectation: sol's insight starts to feel useful after about a week, not the third or fourth week.

## [0.6.14] - 2026-06-25

### Added
- pairing a new device to your journal now matches how you're pairing it. pick phone, computer, or glasses, and solstone shows the right hand-off for each: a large QR code for glasses to point at, a copy-to-clipboard link for a computer, and the usual flow for a phone. and when your journal can be reached more than one way, you can pick which to use.
- solstone can now tidy up its own behind-the-scenes logs and caches on a schedule, so they don't pile up over time. in your storage settings there's a cleanup that removes only these dated housekeeping files (health logs, run logs, and similar caches) once they age out, on by default at 30 days. it never touches what sol has taken in or anything in your memories, only solstone's own bookkeeping. you can preview exactly what a cleanup would remove before anything is deleted, and run it by hand anytime.

### Changed
- your journal's web app is easier to read. text that used to sit on bright facet-colored fills (a selected facet, your chat bubble, the active settings tab) now has stronger contrast, the app matches sol pbc's current look, and a spot where the home screen could get clipped on a narrow window is fixed.
- sol now scopes each lookup in your journal to just the part it needs, rather than reaching across the whole journal at once. this keeps what sol reads tightly scoped, and keeps sol quick to respond even when your journal has grown large.

### Fixed
- a piece of work sol finished could show as failed even though it completed and saved its result. that's resolved; each run now reflects its true outcome.
- when a single activity spanned several facets, each of those facets' history could end up showing the same description copied from one of them. each facet now keeps its own accurate activity history.

## [0.6.13] - 2026-06-24

### Changed
- importing the same thing twice no longer doubles it up in your journal. when you import a file or paste text, whether from the web import page or `sol import`, solstone now recognizes content it has already taken in and skips it instead of creating a second copy. re-running an import that got interrupted picks up where it left off rather than starting over.
- imported items now get the right date straight from the file when solstone can tell. for filenames that already carry a full timestamp, exports from tools that stamp their own times, and media that carries the time it was taken, solstone reads the date directly instead of asking a model to guess it. anything ambiguous still falls back to a best guess as before.

## [0.6.12] - 2026-06-23

### Added
- a new way to run sol on a box that can't keep up with heavy local analysis in real time. pick "deferred" in your processing settings and solstone takes in your day exactly as before, in real time, nothing dropped, while the heavier analysis waits and runs in a window you choose (overnight by default) or while your display is asleep. your journal's health view shows how many segments are waiting and when they last caught up. built for owners running sol on their own hardware, like a single GPU, who want to observe all day and let the deep work happen while the machine is idle.
- while you're in deferred mode, if you ask sol about today and the answer would come up empty, sol now tells you it has taken your day in but hasn't analyzed it yet and will during your chosen window, instead of just coming back with nothing.

### Changed
- your journal's activity records read cleaner. a single moment used to spawn several near-duplicate entries that all shared one summary, including minor ones that didn't warrant their own; now only the ones worth their own entry get one, each described in its own words. existing records are untouched.

### Fixed
- running sol on a local model no longer stalls or comes back empty on a dense, busy day. on constrained hardware a packed day could overflow the local model and produce nothing for that stretch with an opaque error; the local model now sizes itself to your graphics card and fits each request to what it can hold, so a long day finishes cleanly. surfaced by an owner running this on an AMD card.
- the row of active-work dots next to the chat bar reflects what's actually running again. it had been collecting grey "finished" dots and replaying them every time you reloaded or restarted; now it shows only work in flight and clears each item as it finishes.

## [0.6.11] - 2026-06-23

### Changed
- a plain `solstone` install now carries only the thin client commands, `sol` and `solstone`. it no longer puts journal-host commands on your PATH. the `journal` and `mlx-vlm-server` host commands now come with `solstone[journal]` or `solstone[journal-cuda]`, through the new `solstone-journal-host` package. with `uv tool`, install with `uv tool install --with-executables-from solstone-journal-host 'solstone[journal]'`; with pipx, use `pipx install --include-deps 'solstone[journal]'`; with pip, `pip install 'solstone[journal]'` exposes them on its own.
- a couple of settings rough edges are smoothed. naming sol in the sol identity section now commits the name you typed when you press Enter, instead of quietly resetting it to "sol," and a quiet notification in the status-icon list now opens to its full text instead of clipping to a one-line snippet you couldn't expand.
- a day waiting on a local model that's still loading or installing now reads "try again" rather than "a setting's missing," so the days-that-need-a-hand list points you at the right next step.

### Fixed
- observer uploads are being saved to your journal again. on a 0.6.10 install, everything your observers took in returned an error and nothing new landed in your journal; this restores it. if you upgraded to 0.6.10, please install this update.
- on a Mac, your local "how sol thinks" choice sticks. on Apple Silicon it could silently fall back to "no provider chosen" even with a working on-device model; it now reads the actual model state on your Mac. linux is unchanged.
- a partial journal-host install now tells you what to do instead of looping. if the host pieces aren't fully in place, setup stops with reinstall guidance up front rather than starting and failing over and over.

## [0.6.10] - 2026-06-22

### Changed
- the feature that connects all your devices to your journal is now called your private network, in the labels, the setup screens, and the toggle. it's the same encrypted connection among your devices it has always been, just named for what it does. you'll see "your private network" where it used to say "private link."
- the journal backup feature is now called encrypted backup, and its setup copy reads the same way: only you can read your backup, not even sol pbc. nothing about how it works changed, just the name and the wording.
- your health page now reads "all data stored locally on your device" wherever it shows where your journal lives, and notifications now show as built in rather than coming later. the trust details didn't change, the wording is just clearer and current.

### Fixed
- the "minimum requirements" help link now opens a live page. if you're on linux without a supported gpu, the link shown when local models can't run pointed at a page that no longer existed; it now goes to the right support article so you can see what you need.

## [0.6.9] - 2026-06-19

### Fixed
- the error view on your health page now opens the way you'd expect. the "errors today" count jumps you to recent errors every time you click it, not just the first time, and each error row opens in place to show the full plain-language message (and the technical detail when there is one). before, some rows wouldn't open at all. the "api keys" link in settings also now jumps you to the right section on a repeat click.

## [0.6.8] - 2026-06-18

### Added
- you can now import a single image, and sol describes what's in it and adds the entry to your journal. drop in a png, jpeg, webp, gif, or tiff from the web interface or the command line, the same way you import a recording.

### Changed
- chatting with sol no longer makes you wait. when sol kicks off a longer piece of work from a chat, it tells you what it's doing right away and keeps the conversation open, then folds the answer back in once it's ready. the chat shows how many jobs sol has running so you can keep going in the meantime.

### Fixed
- settings now save again. for the past several days, changes you made in settings would quietly revert on reload, with no save button and no error to tell you why. this resolves the underlying cause, so every section saves and sticks.
- finishing the private link setup no longer looks unfinished. when you turned link on and paired a device, a leftover "continue to approve" prompt could linger next to the success message, and the pairing screen could show the wrong text and a code that didn't refresh. the screen now reflects what's actually happening.
- on linux, installing solstone with gpu support no longer fails partway through. some installs pulled a graphics runtime that didn't match the rest of the install, so transcription couldn't load. the install now keeps the pieces in step.
- previewing an import no longer imports it for real. on the command-line importer, asking for a preview while also passing the save flag would import the media live instead of just showing you what it found. a preview now always wins, so you can look before you commit.

## [0.6.7] - 2026-06-18

### Changed
- stretches where nothing on your screen changes (a long overnight, a static window left open) now fold into a single continuation entry in your journal instead of many near-identical ones. sol skips re-describing what hasn't moved, so those stretches process faster and your journal reads cleaner.

### Fixed
- imported recordings now get their own activity record. before, an imported meeting that ran as one continuous stretch could finish without an activity record for it; now it gets one like everything else.
- scheduled tasks that had quietly stopped running now run again on their own, with no action needed from you. an earlier change to how commands are named had left some saved schedules pointing at a form that no longer worked, and this repairs them.
- a settings change you made could be lost if another change landed at the same moment. settings now save one at a time, so concurrent edits all stick.
- your weekly reflection now completes and writes its summary reliably. before, it could spend its whole time budget looking in the wrong place and finish with nothing; it now reads from the right source and lands every time.

## [0.6.5] - 2026-06-17

### Added
- you can mark a single day to be processed again with `journal reprocess DAY --mark-updated`. it re-queues that day even if it already finished, so you can pick up a day that needs another pass without re-running everything.

### Changed
- when sol files something with solstone support on your behalf, you now see and confirm the exact message before it leaves your machine. sol drafts the report, your journal shows you the full contents plus the diagnostic details that would be attached, and nothing is sent until you press send. sol has no path to send on its own; pressing send is the only way anything reaches support, and you can cancel instead and nothing leaves.
- the local web interface no longer has its own password, login page, session cookie, or localhost-trust switch. once setup is complete, the local interface serves directly on the journal machine; linked devices continue to use their paired-device identity through link.
- the connection-and-health indicator now shows the sol ring, with the ring itself carrying live status: whether your journal is reachable, whether observing is healthy, and whether a quiet notification is waiting. it updates continuously while the page is open.

### Fixed
- closed a window during device pairing where someone on your local network could have placed themselves in the middle of the exchange. when a new device pairs with your journal over the local network, it now cryptographically verifies it is talking to your real journal and refuses to continue if anything doesn't match. if you pair devices on shared or untrusted wi-fi, please update.
- pairing a phone or tablet to your journal now works when the journal's machine has more than one network connection. before, the pairing code could advertise an address your other device couldn't actually reach, so the pairing would fail; it now offers every local address so your device can find one that works.
- your journal keeps making progress even when one piece of work stalls. before, a single stuck step (transcribing a clip, thinking through a day, or a model call that never returned) could quietly freeze that part of your journal for hours or days while everything behind it waited; now every piece of work has a time limit, a stall surfaces instead of hiding, and the rest keeps flowing. re-processing a day that had no new activity also no longer loops on itself.
- setting up a hosted service (private link or scout) from the web interface now works when your journal runs on a remote or headless machine. before, the approval step could stall at "setting up…" and never show you the link to continue; it now always gives you a one-tap link to approve.
- installing from source on an aarch64 nvidia machine no longer fails partway through, and transcription works there. if you ran into this, this resolves it.

## [0.6.4] - 2026-06-15

### Added
- `sol link serve --direct` keeps a linked machine's traffic on your private link's direct path and never falls back to the relay — useful when the journal's machine is reachable on the same network, so there's no dependency on the relay.

### Fixed
- an observer on a machine you've linked to your journal can now send everything it takes in over your private link, no matter the size. before, anything larger than a small message stalled partway and never arrived, so those observers couldn't finish uploading and uploads timed out; now they go through reliably. a separate cleanup issue that could leave a stale connection behind after a reconnect is also resolved.

## [0.6.3] - 2026-06-15

### Fixed
- a machine that reaches your journal over your private link now stays connected. before, that connection would go quiet after a minute or two and not recover on its own, so observers on that machine stopped reaching your journal; it now sends a steady heartbeat to stay alive and reconnects automatically if it ever drops, so they keep streaming without interruption.

## [0.6.2] - 2026-06-15

### Changed
- an observer running on another machine you've linked now streams to your journal over your private link. each observer is identified by its own handle, kept fully separate from the machine's link identity, so the two can change independently and one never stands in for the other. observers running on the same machine as your journal are unaffected. what your observers take in, and where your journal lives, are unchanged.

## [0.6.1] - 2026-06-15

### Changed
- desktop observers now use the journal's current callosum connection path,
  with the observer key kept in the authorization header.

## [0.6.0] - 2026-06-14

### Added
- you can now run sol from one machine against a journal that lives on another. once you've paired the two over your solstone private link, `sol chat` and `sol call` reach the remote journal the same way they reach a local one, with live progress and answers streaming back as sol works. a machine set up this way runs its own observers and its own `sol`, while its journal stays on the machine that holds it.
- managing your paired devices now has its own home in `journal link`: list what's paired, check a device's status, and unpair one when you're done with it. `sol link` stays the device-side command for joining and serving a link.

### Changed
- installing solstone now gives you the lightweight `sol` client by default. `pip install solstone` (or `uv tool install solstone`, or `uvx solstone`) installs a small client that talks to a journal running elsewhere, without the full set of components a journal host needs. to run a journal on this machine, install `solstone[journal]` for the full stack, or `solstone[journal-cuda]` for the version that uses your graphics card for transcription. the macOS app is unaffected; it still includes everything to run a journal.
- your journal's local web and api address is now reachable only from the same machine. the older option that opened it to your network behind a password is gone. to use a journal from another device, you now pair that device over your solstone private link, the same pairing flow as before, rather than opening a port. this is a tightening of how a journal can be reached; what your observers take in, where your journal lives, and what leaves the machine are all unchanged.

## [0.5.6] - 2026-06-14

### Added
- the backup page now lets you choose where your encrypted backup lives. keep using your own bucket as before, or pick solstone hosted, which is on the way and shown as coming later with a clear path back to your own bucket in the meantime.

### Changed
- the backup page got a clearer pass over its layout and wording, the routine actions sit apart from the one that erases a backup, and the page now carries the same naming as the rest of solstone, including the `journal backup` terminal command's help text.
- the link page now matches the rest of solstone's look. the pairing button and links on that surface read in the warm palette instead of an off-tone blue, so the page reads as one piece.

### Fixed
- pairing a new device over your solstone private link now completes instead of stalling. before, the join would reach your journal but hang for about half a minute and then give up; it now connects cleanly, with a plain one-line message if anything goes wrong. what travels and where your journal lives are unchanged; this is the connection itself finishing the way it should.
- the recovery-key Copy button on the backup page now copies your key, where before it could copy a stray bit of internal text. some loose setup labels that were never meant to show are gone too.
- when sol's background thinking is cut short at a budget or step limit, that run now closes honestly and its cost shows up on your spending view like any other. before, a cut-short run could sit in limbo and its spend never appeared, so your spending view was lower than what was actually used. a cut-short run now reads plainly as cut short rather than as a clean finish, and keeps whatever partial work it had.

## [0.5.5] - 2026-06-14

### Added
- before sol reaches solstone support on your behalf, it now asks first. reaching support is the one moment something may leave your machine, so sol offers it in the chat bar and waits for your yes; decline and the conversation stays entirely local. when you do say yes, the chat bar shows the reach-out plainly as it happens, and the offer survives a reload so you never lose your place.
- there's a new Thinking app in your journal that owns how sol thinks. provider, key, and on-device setup all live there now, with three clear lanes: become a solstone scout, bring your own key for Claude, Gemini, or GPT, or run local thinking models on your own device. first-run setup now routes you straight into Thinking to pick your lane instead of asking for a single key up front.
- you can now point the local (on-device) option at your own OpenAI-compatible server instead of the bundled engine. when you configure an endpoint, sol's generation and background thinking run there, including screen content for vision; transcription always stays on your machine, and there is no silent fallback to the cloud. it's your server, by your choice, and the local card spells out plainly what goes where.
- linux machines with an arm64 GPU (like the NVIDIA DGX Spark / GB10) can now run the local on-device option, and on-device transcription defaults to whisper on those boxes. previously these setups had no on-device path for sol's thinking.

### Changed
- pairing a new device over your home network is smoother and clearer. a brand-new device on your local network now reaches the pairing step to earn its credentials, the same step it always had to pass; it still has to present your one-shot pairing code, can only talk to the pairing endpoint, and your journal stays local-only until you pair. the pairing command now hands you a ready-to-use link to paste on the other device, and a refused or mistyped attempt now tells you exactly what went wrong instead of a generic error. a paired device also keeps both the name you give it and its own hostname, so they no longer overwrite each other in the list.
- setting up how sol reaches the world is reorganized around the apps that own each piece. Thinking owns scout and your provider setup, Link owns your solstone private link, and the old "your services" switchboard is gone; Settings now opens to a simple guide that points you to each app. the Thinking app also reads scout's real status directly (requested, invited, on), with a "check now" option, instead of guessing from local state.

### Fixed
- the web app's search summary now counts results correctly, reading "1 result" or "2 results" as it should.
- internal stability improvements.

## [0.5.4] - 2026-06-12

### Added
- you can now back up your journal to your own cloud storage, encrypted before it ever leaves your machine. set up a destination you own (an S3 or B2 bucket), and solstone keeps an encrypted copy there on a schedule, with restore and prune built in. you hold the keys: a daily key that runs the backup and a separate recovery key you save somewhere safe, and you need the recovery key to restore. nothing routes through sol pbc, ever; the backup goes straight from your machine to your bucket and back. set it up from the new backup page in your journal, or from the terminal with `journal backup`.
- your journal now has a "your services" page that gathers the optional extras you can turn on, like scout and your private link, in one place. each one shows whether it's on and lets you turn it on or off from inside your journal, with backup linked from there too.
- on-device transcription can now tell voices apart on its own. when the local engine handles your audio, it now labels who's speaking the same way the hosted option already did, so a conversation reads as a back-and-forth rather than one undivided block. this runs entirely on your machine; nothing new leaves it.
- the curation page now suggests when two speaker names look like the same person, alongside the facet and entity merges it already offered, so tidying up who's who has one clear place to happen. it only ever suggests; you decide.

### Changed
- solstone no longer includes todos, skills, routines, or the daily digest. these came out as a deliberate narrowing of what solstone does. anything you'd already saved under them stays untouched on disk; nothing is deleted.
- on Linux, the on-device (local) option now runs on your graphics card instead of the processor. it needs a usable GPU; a Linux machine without one uses a hosted option instead. on macOS, the local option runs on Apple's MLX, as before.
- turning on solstone scout is now a single sign-in link that works the same on every machine, including headless ones. the older code-based path could stall or report a confusing failure on some setups; that path is gone.

### Fixed
- a computer running more than one observer no longer blends them into a single stream. if you ran, say, a desktop observer and a terminal observer on the same machine, they could collapse into one and overwrite each other; each observer now registers itself and keeps its own identity. this happens locally on your machine; nothing new leaves it.

## [0.5.3] - 2026-06-10

### Added
- whether your journal is reachable over your local network is now a setting in journal settings, alongside everything else. it used to live only behind a command-line flag. the default is unchanged: your journal stays local-only until you turn this on.

### Changed
- your journal's own config is now the single source for your managed provider keys. a key set only in your shell environment, and not in your journal config, is no longer picked up. this keeps where your keys come from predictable, and your journal config the one place that decides. nothing about what leaves your machine changes.
- the words across health, settings, and sol now read "talents" where they used to say "agents," with a few labels clarified along the way (disk usage, error counts, and a stray "None" that no longer shows up on the command line).

### Fixed
- search in the web app works again. every search was failing with a generic error; this resolves the underlying cause, and results come back as they should.
- importing a transfer or peer archive now refuses anything that would write outside your journal folder. a malformed or hostile archive is turned away whole, writing nothing — your journal folder stays the only place an import can land.
- marking a todo done or cancelled when it was already finished is now refused with a clear message, instead of leaving it recorded as both at once.
- an entity active right now no longer shows "Last active: Tomorrow" in the evening. the day is computed on your local time, so it reads correctly.
- the chat box no longer shows a provider-setup notice as its placeholder on every page, and a day with nothing in it now gets the same friendly empty state as the other timeline views.
- on macOS, installing or upgrading the journal service no longer fails on a slow machine. some upgrades could stop on an input/output error while the old service was still unloading; this now waits and retries instead of giving up.

## [0.5.2] - 2026-06-05

### Changed
- terminal `sol chat` now runs through the same chat that the web app uses, so you get the same answer whichever way you ask. it shows its progress as it works and prints the final answer at the end.

### Fixed
- on-device transcription no longer splits words apart. transcripts from the local engine were coming through with letters scattered ("Tak ing out the m one y" instead of "Taking out the money"); new transcriptions now read as natural words and sentences. anything already stored garbled needs a fresh re-transcription to clean up.
- asking sol about your own journal now gets an answer. questions that look something up in your memory, like past conversations, your notes, a quote, or your name, used to get declined; sol answers them now. reflection-style questions are unchanged.
- on macOS, setup and upgrade no longer stall at the readiness check. a diagnostic step could take long enough to time out on a fresh or cold machine; it now runs in a fraction of a second.
- syncing a folder of audio now tells you what it couldn't read. `sol import --sync audio` used to fold an unreadable file in with the ones it intentionally skipped; an unreadable file is now called out on its own line, with a reason, and the per-file list shows up with `-v`. `--auto` guidance also works alongside `--sync --save` now.
- on Linux, installing or reinstalling the journal service no longer removes other services you set up. if you had your own solstone-related units, like a desktop observer or your own backup job, an upgrade could delete them. that cleanup step is gone.

## [0.5.1] - 2026-06-05

### Fixed
- macOS installs now get the same clean journal package as every other platform. the Apple Silicon wheel no longer carries retired internal talent prompts from an earlier build, so provider readiness stays available after install.

## [0.5.0] - 2026-06-05

### Added
- you can now point solstone at a folder of audio files and keep it in sync with your journal. `sol import --sync audio --path <folder>` shows what is new, and `--save` imports only what has not already been brought in.
- sol now surfaces suggested facets and entity merges for review, so organizing the journal has a clear place to happen.

### Changed
- provider readiness is now one signal across Settings, Health, support diagnostics, chat, and the command line. when a provider or model is blocking work, solstone shows the affected task and the next step instead of scattering the reason across pages.
- on Apple Silicon, the local setup path now matches the on-device engine the journal actually runs, including the right memory requirement before activation.
- transcription setup is more careful about memory. when the local model is a poor fit for the machine, solstone says so up front and points to a hosted option instead of trying to push through.

### Fixed
- provider settings load cleanly from a direct link and show each task's defaults.
- audio folder sync retires missing skipped files when the source folder changes, so a dry run does not keep warning about files that are no longer there.

## [0.4.10] - 2026-06-02

### Added
- pdfs and still images that come in with your day are now read into your journal and made searchable, the same way a transcript is. sol can find what a document or an image actually contains, not just that one was there. on a scanned pdf with no selectable text, sol reads the page itself. nothing new leaves your machine, and what's kept on disk is unchanged.

### Fixed
- if you run the on-device option on a Mac, it works again. on apple silicon, on-device thinking and vision had started turning away every request, so the day's processing stalled; this resolves it. and if the on-device engine ever restarts, your journal now catches up on anything it missed while it was down.
- solstone no longer tells you an update is available when there isn't one. if you were already on the latest build, or running a pre-release or development build that's ahead of it, the check could nudge you for nothing. it now points to an update only when a genuinely newer published version exists.

## [0.4.9] - 2026-06-02

### Fixed
- upgrading on macos is steadier when the journal service is already running. setup waits for the old service to finish unloading and uses the service's real start time, so a healthy upgrade does not stall at readiness.
- solstone raises the file limit for the installed journal service, giving long-running journals more room for observers, local providers, and background work.
- revealing a provider key in settings is now only visual. clicking the eye button no longer triggers an unwanted save, validation, or clear.

## [0.4.8] - 2026-06-01

### Fixed
- macos setup now tolerates a launchd race where the journal service starts and becomes healthy even though `launchctl kickstart` reports a transient error. setup trusts the supervisor readiness marker, so an upgrade can continue to observer registration instead of stopping at "service up failed".

## [0.4.7] - 2026-05-31

### Fixed
- upgrading over an older install no longer stops because the `sol` or `journal` shortcut in your shell points somewhere stale. setup now repairs the shortcuts it owns and keeps going, whether solstone came from the macos app or from the terminal.
- sol's background thinking can ask the journal for identity, routines, health, and talent context again. those approved journal tools were being turned away before sol could use them; now they work without widening what sol is allowed to run.
- fresh installs from PyPI resolve cleanly when pip chooses the dependencies. solstone now pins the matching telemetry packages used by sol's thinking runtime, so install no longer lands on an incompatible mix.

## [0.4.6] - 2026-05-31

### Added
- you can now re-run sol's thinking on any day from the page, either "process now" to pick up where it left off or "redo from scratch" to start the day over. the same is available in the terminal with `sol reprocess <day>`.

### Changed
- your journal now tells you plainly whether it's caught up. the stats and health pages show an honest "is my journal caught up?" answer plus a "days that need a hand" list for any day it can't finish on its own, like one with corrupted data or a step that keeps failing. catch-up runs on its own in the background, never leaves older days behind, and `journal doctor` reports the same answer from the terminal.
- the on-device option is now a single "Local (on-device)" choice on both macOS and linux. on a Mac it now runs entirely on your machine, including sol's thinking, so the local-only path covers more of your journal without anything leaving your machine.
- on linux, the default on-device transcription now works the moment you install, with no extra to add. the runtime ships with the install and `journal setup` fetches the model. owners with an NVIDIA GPU can still opt into a GPU-accelerated build, and `journal doctor` now reports whether the default transcription runtime and model are ready.
- local journal commands now live with the journal service. things like navigating, routines, identity, on-device provider install, health, and stats moved from `sol` to `journal` (for example `journal navigate`, `journal health`, `journal reprocess`). the old `sol` forms now point you to the new one.

### Fixed
- your journal no longer shows finished work as still pending. days that had an earlier error but later completed were being counted as outstanding, so the backlog looked larger than it was. the count now reflects what's actually still incomplete.

## [0.4.5] - 2026-05-30

### Added
- you can now reach your journal from your phone or laptop even when they aren't on the same network as your home machine. setup lives at the connections page, which is now the single home for how you connect, your network access, and your paired devices. pairing shows a fresh code, lets you name each device, and lets you see and remove any device with one tap.
- the local model that runs entirely on your machine can now take in images as well as text, so the on-device option works on more of what's in your journal. nothing new leaves your machine.

### Changed
- the local model is now kept running for you in the background instead of starting up on demand, so it's ready the moment sol needs it. fresh installs launch it reliably the first time, and a model download now shows real progress instead of sitting at 0 percent through several gigabytes.
- diagnostics are now two clearer commands. `sol doctor` checks that the `sol` command itself is working, from anywhere. the new `journal doctor` checks the health of your journal and its background service. each asks only the question that fits where you run it, so neither raises a false alarm.
- the entities and devices views read more clearly: plain empty states when there's nothing yet, a retry when something fails to load, and detaching a facet now spells out what will happen and offers a one-tap way to undo it.

### Fixed
- your journal now shows when a moment has been transcribed but not yet thought through, instead of looking finished, and catches those moments up on its own. day-by-day status and the transcripts view reflect this honestly, so nothing sits half-processed without you knowing.

## [0.4.4] - 2026-05-27

### Changed
- when sol is catching up on a backlog, today's thinking no longer waits in line behind it. on a busy journal, or right after an install, the day's catch-up work and sol's thinking on fresh observations now run alongside each other, so new moments get attended to in seconds instead of waiting hours.

### Fixed
- transcripts come through on every audio format again. if you run a transcription backend other than whisper, some audio was making it into your journal but quietly producing no transcript. this resolves it, so the moments you spoke are written down the way you'd expect.
- upgrading from an older install no longer trips a setup check. if you first installed solstone a different way and then moved to the current method, `sol doctor` now adjusts the older `sol` and `journal` shortcuts for you instead of stopping. if you hit this, this resolves it.

## [0.4.3] - 2026-05-27

### Added
- a dedicated reader for facet newsletters at `/app/news/`. reverse-chrono index across all your facets, per-day detail with a copy button and a pdf download, and a sample newsletter so you can see the shape before your first one lands. newsletters sit next to reflections in the sidebar.

### Changed
- the participation tab on an activity now shows a structured list of people, grouped into attendees and mentioned, with a short note next to each name about how that person showed up in the activity. low-confidence entries appear muted with a "less certain" tag, and empty or unavailable states read in plain language instead of raw json.

### Fixed
- weekly reflection writes a full reflection to your journal again. on busy journals it was running out of room mid-gather and either saving nothing or saving only a short summary; both paths are resolved, and the reflections page renders again.
- attendee lists are stricter about who counts as an attendee. someone whose name only appeared in a transcript, without other corroboration, is now demoted to mentioned rather than surfaced as an attendee. reported by Ryan during a walkthrough.
- background work sol runs through google (morning briefings and other scheduled talents) no longer fails silently on a size limit. a request-budget calculation was landing one over the supported maximum, rejecting every call on the default settings; the calculation is corrected.
- sidebar labels in the expanded menu no longer truncate. entities, transcripts, and other longer labels show in full at narrower window widths. reported by Ryan.

## [0.4.2] - 2026-05-26

### fixed
- on a fresh install, `journal setup` could stop on a doctor check that flagged the `sol` command on your machine as out of place — even when it was the one journal had just put there. if you hit this setting up 0.4.1, this resolves it.

## [0.4.1] - 2026-05-26

### fixed
- some 0.4.0 installs didn't come back up after upgrading — sol wouldn't start, and journal commands timed out. this resolves it.

## [0.4.0] — 2026-05-26

### changed
- **service commands moved fully to `journal`.** Service commands (supervisor, cortex, heartbeat, setup, transcribe, services, etc.) are no longer surfaced under `sol` — they live exclusively under `journal`. Your existing solstone service migrates itself automatically on the next service restart; no action needed.
- `journal start` is now the canonical run command (replaces `journal supervisor` as the service-unit entry point — old units self-migrate).
- the `sol` CLI continues to be your day-to-day surface (chat, call, top, import, search across the journal).
- cogitate is now baseline — the openhands-sdk runtime that powers sol's tool-calling agents ships in the wheel, so a fresh install with a hosted provider key runs cogitate immediately with no extra install step. wheel size grows by about 337 MB on install to carry openhands-sdk, litellm, and their transitive dependencies.
- minimum python is now 3.12 (was 3.11) — required by the openhands-sdk runtime that ships baseline. if you installed solstone with a 3.11 interpreter, reinstall under 3.12+ before updating.

### removed
- `sol <service-cmd>` paths typed by a human now redirect to `journal <cmd>` with a clear error and exit non-zero. Service units still pointing at the old paths self-migrate; nothing on disk breaks.
- the built-in `sol observer install` command is gone. linux and tmux observers now install from their own published packages: `pipx install solstone-linux` (or `solstone-tmux`), then `solstone-linux install-service` (or `solstone-tmux install-service`) pairs with the running journal. the macOS observer continues to come from the signed app bundle at solstone.app/observers.
- the bundled per-provider install commands are gone — `sol call settings providers install` now accepts `local` only (cogitate runs out of the box for hosted providers with a key set), and `uninstall`/`disable`/`enable`/`validate-key` are removed entirely. local install continues to work via `sol call settings providers install local`.

## [0.3.10] — 2026-05-26

### Added
- **journal CLI** — `solstone` now installs two CLI binaries: `sol` (the day-to-day surface for talking to your journal — chat, call, top, import, etc.) and `journal` (host operations — supervisor, setup, install-models, the daemons that tend your journal). `sol --help` shows both surfaces; `journal --help` shows just the host commands. Existing `sol <cmd>` invocations all keep working. Internal docs and scripts use `journal <cmd>` for host operations going forward.

## [0.3.9] - 2026-05-25

### Added
- solstone now has a "services" layer for the optional cloud-backed extras sol pbc offers alongside your local solstone. today that means solstone scout, the alpha-tester program that provisions a Google Gemini key for you and unlocks scout-only features. services are off by default; you turn them on from `services.solstone.app` or `sol services enable scout`, and solstone itself still runs entirely on your machine.
- you can now move days or whole journals between your own machines, and connect an observer on one machine to a journal on another, over a direct private link between your devices. `sol link join` pairs them; `sol transfer send --to <peer>` and `sol export --to <peer>` push from one to the other. revoking a paired device at the `/link` dashboard, with `sol observer revoke`, or with `sol call link unpair` cuts the connection at TLS the moment you revoke.
- a new "Local (on-device)" provider runs sol from a bundled `llama-server` on your own machine with a pinned Qwen model. zero-egress: when sol is set to local, it never falls through to a cloud provider.
- a new daily `journal/identity/health.md` surface tells you whether solstone is OK at a glance. sol reads its own signals, auto-recovers from things like stuck transcripts, and the home page and morning briefing now read its summary.

### Changed
- a few of the surfaces you touch most are now more direct. creating a facet lands you on a real detail page that confirms what you just made and offers next steps. clicking a "needs you" item on the home page opens a fresh chat with editable starter text already in the box (not as ghost placeholder), and sol knows which item you came from. each modality on segment-detail pages has its own "analyze now" button so you can re-run analysis on one part of a day without dropping to a terminal. the health, tokens, and service-log pages were rebuilt around a glance row that answers the first question (is solstone OK, is this costing too much, where did the pipeline fail) with the detail kept under progressive disclosure; service log lines now carry severity colors with screen-reader announcements on errors.

### Fixed
- segments that were already analyzed sometimes painted as still-pending on the day timeline; they now render correctly. audio playback on segment pages now shows the real duration and the right format, transcript lines no longer carry a doubled timestamp, the day view scrolls naturally on short windows, and a cold-load race on transcripts pages is resolved. internal stability improvements across providers install, background tasks, and the convey wizard.

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
