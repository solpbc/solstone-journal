journal.cli.streams.arguments = usage: { $syntax }{ $break }{ $command }: error: unrecognized arguments: { $arguments }

journal.cli.streams.empty = No streams found

journal.cli.streams.header.day = Last Day

journal.cli.streams.header.name = Name

journal.cli.streams.header.segment = Last Segment

journal.cli.streams.header.sequence = Seq

journal.cli.streams.header.type = Type

journal.cli.streams.help = usage: { $syntax }{ $paragraph }Inspect and manage stream identity{ $paragraph }positional arguments:{ $break }{ $indent }{ $name_input }{ $name_space }Stream name to inspect (omit to list all streams){ $paragraph }options:{ $break }{ $indent }{ $help_input }{ $help_space }show this help message and exit{ $break }{ $indent }{ $rebuild_input }{ $rebuild_space }Reconstruct stream state from per-segment markers{ $break }{ $indent }{ $verbose_input }{ $verbose_space }Enable verbose output{ $break }{ $indent }{ $debug_input }{ $debug_space }Enable debug logging

journal.cli.streams.inspect.malformed = Could not read stream { $path }: malformed record

journal.cli.streams.inspect.not.found = Stream not found: { $name }

journal.cli.streams.list.error = Could not list streams: { $error }

journal.cli.streams.marker.sequence = unreadable marker { $path }: invalid sequence

journal.cli.streams.marker.unreadable = unreadable marker { $path }: could not read marker

journal.cli.streams.rebuild.empty = { $segment_quantity ->
    [one] No streams found ({ $segments } segment scanned)
   *[other] No streams found ({ $segments } segments scanned)
}

journal.cli.streams.rebuild.result = { $stream_quantity ->
    [one] Rebuilt { $streams } stream from { $segment_quantity ->
        [one] { $segments } segment:
       *[other] { $segments } segments:
    }
   *[other] Rebuilt { $streams } stream(s) from { $segment_quantity ->
        [one] { $segments } segment:
       *[other] { $segments } segments:
    }
}

journal.cli.streams.record.malformed = Could not read stream record { $path }: malformed record

journal.cli.streams.repair.ahead = { $stream }: stream state sequence is ahead of markers; unchanged

journal.cli.streams.repair.locked = { $stream }: could not lock { $path }; unchanged

journal.cli.streams.repair.malformed = { $stream }: malformed stream record at { $path }; unchanged

journal.cli.streams.repair.missing = { $stream }: no stream record at { $path }; unchanged

journal.cli.streams.repair.write.failed = { $stream }: write failed for { $path }: atomic publication failed

journal.cli.streams.segments.error = Could not list segments: { $error }

journal.cli.streams.supervisor = journal isn't running. start it with '{ $command }' and retry.

journal.cli.doctor.context.failed = doctor failed to resolve context: { $error }

journal.cli.doctor.help = usage: { $syntax }{ $line_break }{ $line_break }Run { $brand } diagnostics.{ $line_break }{ $line_break }options:{ $line_break }{ $indent }{ $help_option }{ $help_gap }show this help message and exit{ $line_break }{ $indent }{ $verbose_option }{ $verbose_gap }print every check result{ $line_break }{ $indent }{ $json_option }{ $json_gap }emit JSON instead of text{ $line_break }{ $indent }{ $jsonl_option }{ $jsonl_gap }emit one-JSON-per-line events instead of text{ $line_break }{ $indent }{ $port_option }{ $port_gap }port to probe (default: { $port }){ $line_break }{ $indent }{ $readiness_option }{ $readiness_gap }run the setup readiness battery{ $line_break }{ $line_break }If '{ $command }' is unavailable, build the native doctor from this checkout:{ $line_break }{ $build_command }

journal.cli.doctor.usage.argument = usage: { $syntax }{ $line_break }{ $command }: error: unexpected argument

journal.cli.doctor.usage.formats = usage: { $syntax }{ $line_break }{ $command }: error: { $json_flag } and { $jsonl_flag } are mutually exclusive

journal.cli.doctor.usage.port = usage: { $syntax }{ $line_break }{ $command }: error: { $port_flag } requires an integer

journal.cli.doctor.backup.status.unconfigured = backup not configured

journal.cli.doctor.backup.status.off = backup off

journal.cli.doctor.backup.status.no.attempt = backup on; no recorded attempt

journal.cli.doctor.backup.status.invalid.settings = backup status unavailable: invalid backup settings

journal.cli.doctor.backup.status.invalid.switch = backup status unavailable: invalid on/off setting

journal.cli.doctor.backup.status.invalid.pause = backup status unavailable: invalid pause setting

journal.cli.doctor.backup.status.unavailable.journal = backup status unavailable: journal unreadable

journal.cli.doctor.backup.status.unavailable.settings = backup status unavailable: settings unreadable

journal.cli.doctor.capture.health.inspect = open { $health_path } to inspect each device

journal.cli.doctor.capture.health.active = { $rollup }; the { $brand } app on every device that has added to your journal is current

journal.cli.doctor.capture.health.no.senders = { $rollup }; the { $brand } app hasn't added anything to your journal yet

journal.cli.doctor.capture.health.quiet = { $rollup }; no rejected uploads recorded; see devices for last delivery times

journal.cli.doctor.capture.health.data.unavailable = { $rollup }; device activity unavailable

journal.cli.doctor.capture.health.registry.unavailable = { $rollup }; device list unavailable

journal.cli.doctor.client.binding.active = active client records={ $records }; unbound=0; { $certificate_term } binding is authoritative

journal.cli.doctor.client.delivery.stall.activity.unavailable = device activity unavailable

journal.cli.doctor.client.delivery.stall.list.unavailable = device list unavailable

journal.cli.doctor.client.delivery.stall.quiet = no rejected uploads recorded; see devices for last delivery times

journal.cli.doctor.client.delivery.stall.inspect = open { $health_path } to inspect the rejected upload

journal.cli.doctor.client.delivery.stall.no.senders = the { $brand } app hasn't added anything to your journal yet

journal.cli.doctor.client.ingest.health.rejection = device { $device } failing ingest: { $reason }, { $attempts }x since { $since }

journal.cli.doctor.client.ingest.health.healthy = no devices failing ingest

journal.cli.doctor.client.ingest.health.repair = update or restart the device, then confirm a valid upload clears the rejection

journal.cli.doctor.client.transport.refusal.healthy = no devices had requests turned away in the last 7 days

journal.cli.doctor.client.transport.refusal.repair = this is a busy connection, not a lost one; report it if it keeps happening

journal.cli.doctor.config.dir.readable.owner.unavailable = could not locate the owner base: { $error }

journal.cli.doctor.config.dir.readable.config.writable = home and service config dir are writable ({ $path })

journal.cli.doctor.config.dir.readable.home.missing = home directory does not exist: { $path }

journal.cli.doctor.config.dir.readable.home.unusable = home directory is not readable and writable: { $path }

journal.cli.doctor.config.dir.readable.config.will.create = home is writable; install will create { $path }

journal.cli.doctor.config.dir.readable.config.unwritable = service config directory is not writable: { $path }

journal.cli.doctor.config.dir.readable.owner.windows = the Windows owner base is only readable on Windows

journal.cli.doctor.config.restore = repair or restore { $config_file } from a backup

journal.cli.doctor.default.stt.ready.backend = configured backend is { $backend }; parakeet readiness not applicable

journal.cli.doctor.default.stt.ready.download = { $coreml } parakeet model is not downloaded — fetch it with: { $models_command }

journal.cli.doctor.default.stt.ready.other.model.ready = parakeet model ready at { $path }

journal.cli.doctor.default.stt.ready.unsupported = parakeet not supported on this platform

journal.cli.doctor.device.day.listing.repair = a device syncing that day is told the journal cannot read it; a { $brand } app older than the 2026-09 fix shows that as offline; inspect the named stream directory's segments and their { $events_file }

journal.cli.doctor.device.day.listing.fault = day { $day } stream { $stream } device { $device }: { $reason }

journal.cli.doctor.device.day.listing.streams.unavailable = device streams unavailable: { $error }

journal.cli.doctor.device.day.listing.streams.unbound = no device streams bound

journal.cli.doctor.device.day.listing.readable = the journal can read the last { $days } days for every bound device

journal.cli.doctor.devices.absent = no registered devices

journal.cli.doctor.devices.unavailable = device records unavailable: { $error }

journal.cli.doctor.disk.space.package.install = a package install is replaced in place when it updates, so no second copy is needed

journal.cli.doctor.disk.space.space.inspect = could not inspect free space at { $path }: { $error }

journal.cli.doctor.disk.space.size.unreadable = could not measure installation tree at { $path }: { $error }

journal.cli.doctor.disk.space.root.unresolved = could not resolve installation root from install bin directory: { $path }

journal.cli.doctor.disk.space.space.unreadable = free space is not readable at { $path }

journal.cli.doctor.disk.space.space.available = { $free_gib } { $gib } free on the filesystem holding { $path } ({ $required_gib } { $gib } is a second copy of this install plus headroom)

journal.cli.doctor.disk.space.size.overflow = installation tree size overflow at { $path }

journal.cli.doctor.disk.space.space.low = only { $free_gib } { $gib } free on the filesystem holding { $path }; { $required_gib } { $gib } is a second copy of this install plus headroom

journal.cli.doctor.disk.space.layout.unsupported = resolved installation root is not a layout install tree

journal.cli.doctor.journal.absent = no local journal

journal.cli.doctor.journal.caught.up.coverage.unavailable = couldn't determine daily unit coverage on { $day }: { $error }

journal.cli.doctor.journal.caught.up.backlog.unavailable = couldn't fully determine; backlog read failed: { $error }

journal.cli.doctor.journal.caught.up.definitions.unavailable = couldn't fully determine; couldn't locate the daily processing definitions: { $error }

journal.cli.doctor.journal.caught.up.facets.day.unavailable = couldn't fully determine; failed to check active facets for { $day }: { $error }

journal.cli.doctor.journal.caught.up.chronicle.unavailable = couldn't fully determine; failed to read chronicle: { $error }

journal.cli.doctor.journal.caught.up.chronicle.entry.unavailable = couldn't fully determine; failed to read chronicle entry: { $error }

journal.cli.doctor.journal.caught.up.configs.unavailable = couldn't fully determine; failed to read daily configs: { $error }

journal.cli.doctor.journal.caught.up.units.entry.unavailable = couldn't fully determine; failed to read daily-unit entry for { $day }: { $error }

journal.cli.doctor.journal.caught.up.units.unavailable = couldn't fully determine; failed to read daily-units directory for { $day }: { $error }

journal.cli.doctor.journal.caught.up.facets.unavailable = couldn't fully determine; failed to read facets: { $error }

journal.cli.doctor.journal.caught.up.unit.invalid = couldn't fully determine; malformed daily unit file { $file_name } on day { $day }: { $source }

journal.cli.doctor.journal.caught.up.unit.unreadable = couldn't fully determine; unreadable daily unit file { $file_name } on day { $day }: { $source }

journal.cli.doctor.journal.caught.up.unit.identity = couldn't read daily unit file { $file_name } on day { $day }: record identity does not match its path

journal.cli.doctor.journal.caught.up.unit.version = couldn't read daily unit file { $file_name } on day { $day }: unsupported record version { $version }

journal.cli.doctor.journal.caught.up.outside.backlog = daily unit outside the recent backlog on { $day }

journal.cli.doctor.journal.caught.up.inspect = open health to see waiting work and any spending confirmation; reprocess a past day to give it priority after today

journal.cli.doctor.journal.caught.up.inventory.fix = open health to see waiting work and its next step

journal.cli.doctor.journal.caught.up.repair = re-run { $doctor_command }; check the health logs if it persists

journal.cli.doctor.journal.caught.up.inventory.waiting = whole-journal inspection has not reported yet

journal.cli.doctor.journal.caught.up.inventory.unavailable = whole-journal inspection unavailable: { $error }

journal.cli.doctor.journal.dir.writable.parent.unwritable = journal dir absent; nearest existing ancestor is not writable: { $path }

journal.cli.doctor.journal.dir.writable.parent.writable = journal dir absent; parent { $path } is writable

journal.cli.doctor.journal.dir.writable.unwritable = journal dir not writable: { $path }

journal.cli.doctor.journal.dir.writable.writable = journal dir writable: { $path }

journal.cli.doctor.journal.dir.writable.not.directory = journal path exists but is not a directory: { $path }

journal.cli.doctor.journal.dir.writable.move = move or remove { $path }, then re-run

journal.cli.doctor.journal.sources.readable.inspect.folders = check the day folders named above — a name that is not a calendar date is not a day the journal can process

journal.cli.doctor.journal.sources.readable.stats.missing = couldn't check — no statistics to read

journal.cli.doctor.journal.sources.readable.stats.invalid = couldn't check — the statistics aren't readable

journal.cli.doctor.journal.sources.readable.stats.predates = couldn't check — the statistics predate this version

journal.cli.doctor.journal.sources.readable.repair.files = repair the file named above, then run { $reprocess_command } for each listed day that has already ended

journal.cli.doctor.journal.sources.readable.reprocess = run { $reprocess_command } for each listed day that has already ended

journal.cli.doctor.journal.sources.readable.healthy = the last scan found no unreadable days

journal.cli.doctor.launchd.stale.plist.invalid = could not parse plist: { $plist_error }: { $error }

journal.cli.doctor.launchd.stale.plist.absent = launchd plist absent

journal.cli.doctor.launchd.stale.plist.target.exists = launchd plist target exists ({ $path })

journal.cli.doctor.launchd.stale.plist.arguments.missing = plist is missing { $plist_argument }

journal.cli.doctor.launchd.stale.plist.target.missing = plist points to missing executable: { $path }

journal.cli.doctor.launchd.stale.plist.repair = run { $uninstall_command }, then run { $install_command } separately to reinstall a headless background service

journal.cli.doctor.local.bin.solstone.reachable.file.missing = { $path } is missing

journal.cli.doctor.local.bin.solstone.reachable.file.invalid = { $path } is not a file

journal.cli.doctor.local.bin.solstone.reachable.path.ready = { $wrapper_path } is on { $path_variable } at { $path }

journal.cli.doctor.local.bin.solstone.reachable.symlink.ready = { $wrapper_path } symlinks to { $path_variable } { $brand } at { $path }

journal.cli.doctor.local.bin.solstone.reachable.windows.terminal = open a new terminal window and try again; if { $brand } is still not found, reinstall the journal

journal.cli.doctor.local.bin.solstone.reachable.path.mismatch = { $path_variable } { $brand } resolves to { $resolved }, expected { $expected }

journal.cli.doctor.local.bin.solstone.reachable.windows.path.priority = put { $install_dir } ahead of { $other_dir } on your { $path_variable }

journal.cli.doctor.local.bin.solstone.reachable.path.repair = put { $bin_dir } earlier on { $path_variable }, or run { $setup_command } to repoint the managed wrapper

journal.cli.doctor.local.bin.solstone.reachable.windows.repair = reinstall the journal to restore { $executable }

journal.cli.doctor.local.bin.solstone.reachable.wrapper.install = run { $setup_command } to install the managed { $brand } wrapper

journal.cli.doctor.local.bin.solstone.reachable.path.absent = { $brand } is not on { $path_variable }

journal.cli.doctor.local.bin.solstone.reachable.windows.ready = { $brand } is on { $path_variable } at { $path }

journal.cli.doctor.orphan.segment.pdf.repair = add a readable { $transcript_file } beside the PDF original, then re-run { $doctor_command }

journal.cli.doctor.orphan.segment.pdf.clear = all raw PDF originals have readable document transcripts

journal.cli.doctor.orphan.segment.pdf.chronicle.unavailable = chronicle directory unavailable

journal.cli.doctor.parakeet.cpp.stt.ready.backend.not.selected = configured backend is not { $backend_name }; check not applicable

journal.cli.doctor.parakeet.cpp.stt.ready.runtime.install = install the system { $openmp } runtime that provides { $runtime_file }, then rerun { $doctor_command }

journal.cli.doctor.parakeet.cpp.stt.ready.install = { $backend_name } artifacts are not installed — fetch them with: { $provider_command }

journal.cli.doctor.parakeet.cpp.stt.ready.binary.unavailable = { $backend_name } binary cannot start

journal.cli.doctor.parakeet.cpp.stt.ready.runtime.unavailable = { $backend_name } cannot start: { $openmp } runtime unavailable ({ $runtime_file })

journal.cli.doctor.parakeet.cpp.stt.ready.unsupported = { $backend_name } is only supported on linux and windows

journal.cli.doctor.parakeet.cpp.stt.ready.ready = { $backend_name } ready (binaries + model installed, server reachable)

journal.cli.doctor.parakeet.cpp.stt.ready.online = { $backend_name } ready (server + model match the signed package, server reachable)

journal.cli.doctor.parakeet.cpp.stt.ready.start = { $server } is not reachable — start the journal service: { $start_command }

journal.cli.doctor.parakeet.cpp.stt.ready.unreachable = { $server } not reachable: { $error }

journal.cli.doctor.parakeet.cpp.stt.ready.server.missing = { $server } not reachable: your journal is running, but not { $server }

journal.cli.doctor.parakeet.cpp.stt.ready.offline = { $server } not reachable: your journal isn't running

journal.cli.doctor.path.repair = fix ownership/permissions of { $path }

journal.cli.doctor.sense.dispatch.clear = no Sense dispatch errors since the last successful handler sync

journal.cli.doctor.sense.dispatch.count.out.of.range = Sense status is incomplete: dispatch error count is outside its supported range

journal.cli.doctor.sense.dispatch.count.invalid = Sense status is incomplete: no valid nonnegative dispatch error count

journal.cli.doctor.service.absent = no local journal service

journal.cli.doctor.service.identity.repair.force = run { $setup_command } from this install to refresh the service

journal.cli.doctor.service.identity.repair = run { $setup_command } to reinstall the service

journal.cli.doctor.service.identity.target.current = service target matches current install: { $raw } -> { $path }

journal.cli.doctor.service.identity.windows.current = service target matches current install: { $command }

journal.cli.doctor.service.identity.windows.mismatch = service target mismatch: { $mismatch }

journal.cli.doctor.service.identity.target.mismatch = service target mismatch: { $raw } resolves to { $resolved }, expected { $expected }

journal.cli.doctor.service.identity.target.absent = service unit is malformed: no executable target in { $path }

journal.cli.doctor.service.registration.unreadable = couldn't read the journal's service registration — { $reason }

journal.cli.doctor.service.running.running = journal service is running

journal.cli.doctor.service.running.unit.failed = journal service unit is failed

journal.cli.doctor.service.running.registration.no.reader = no reader for the service registration

journal.cli.doctor.service.running.pid.invalid = probe child PID does not fit a process-group ID

journal.cli.doctor.service.running.logs = run { $logs_command }

journal.cli.doctor.service.running.restart = run { $restart_command }; if it persists, run { $logs_command }

journal.cli.doctor.service.running.start = run { $start_command }

journal.cli.doctor.service.running.stopped = service installed but not running

journal.cli.doctor.skill.state.link.missing = { $skill } missing at { $path }

journal.cli.doctor.skill.state.link.foreign = { $skill } points elsewhere at { $path }

journal.cli.doctor.skill.state.directories.missing = router skill directories are unavailable

journal.cli.doctor.skill.state.checkout.required = router skill state is only available from a source checkout

journal.cli.doctor.skill.state.links.ready = router skills { $brand }, journal are installed and current

journal.cli.doctor.skill.state.install = run { $skills_command }

journal.cli.doctor.skill.state.link.stale = stale router skill link at { $path }

journal.cli.doctor.supervisor.conflict.clear = no macOS supervisor conflict (native check: foreign-launcher scan only; no competing KeepAlive launcher found)

journal.cli.doctor.supervisor.conflict.unsupported = not supported on windows

journal.cli.doctor.task.pace.about.cap = a job is running long; it will be stopped automatically if it passes its cap — no action needed unless it persists

journal.cli.doctor.task.pace.status.unavailable = supervisor status unavailable

journal.cli.doctor.task.pace.healthy = tasks on pace

journal.cli.doctor.task.queue.holds.clear = all task partition records are clean

journal.cli.doctor.task.queue.holds.absent = no in-flight or held task partition records present

journal.cli.doctor.unretryable.transcribe.input.clear = 0 recordings the journal will not retry on its own (handler: { $handler }, { $reason_key }: { $reason_code })

journal.cli.doctor.unretryable.transcribe.input.open = cannot open { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.read = cannot read { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.directory.unreadable = cannot read segment dir { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.entry.unreadable = cannot read segment entry in { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.scan.failed = could not determine how many recordings the journal will not retry on its own ({ $reason })

journal.cli.doctor.unretryable.transcribe.input.segments.unreadable = failed to list segments for day { $day }: { $error }

journal.cli.doctor.unretryable.transcribe.input.days.unreadable = failed to read chronicle days: { $error }

journal.cli.doctor.unretryable.transcribe.input.row.unavailable = first row in { $path } cannot be established or exceeds byte limit

journal.cli.doctor.unretryable.transcribe.input.row.large = first row in { $path } exceeds byte limit

journal.cli.doctor.unretryable.transcribe.input.row.json = first row in { $path } is not valid json: { $error }

journal.cli.doctor.unretryable.transcribe.input.row.encoding = first row in { $path } is not valid utf-8: { $error }

journal.cli.doctor.unretryable.transcribe.input.identity.invalid = invalid segment identity in { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.handler.missing = processing record in { $path } missing handler field

journal.cli.doctor.brain.age.unknown = unknown

journal.cli.doctor.brain.no.component = none

journal.cli.doctor.brain.report = { $headline }; { $state_key }={ $state }; { $reason_key }={ $reason }; { $component_key }={ $component }; { $age_key }={ $age }

journal.cli.doctor.brain.unknown = unknown: brain state unavailable

journal.cli.doctor.brain.unknown.reason = unknown: { $reason }

journal.cli.doctor.index.fix.checking = search is checking this itself and rebuilds if the damage is confirmed; run { $command } for details

journal.cli.doctor.index.fix.deletions = search keeps retrying these removals; run { $command } for details

journal.cli.doctor.index.fix.errors = run { $command } and check its errors

journal.cli.doctor.index.fix.failed.files = search retries these updates at its daily check; run { $command } for details

journal.cli.doctor.index.fix.failed.work = search retries this work every hour; run { $command } for details

journal.cli.doctor.index.fix.full = run { $command }

journal.cli.doctor.index.fix.rebuild = search rebuilds its index from your journal once the damage is confirmed; if it persists, run { $command }

journal.cli.doctor.index.fix.reset = run { $command }

journal.cli.doctor.index.fix.upgrade = update { $brand } to the newest version

journal.cli.doctor.index.fix.warnings = run { $command } and check its warnings

journal.cli.doctor.storage.privacy.choice = other accounts on this computer may be able to read your journal. its folder could not be made private on this drive. you can keep it on this drive or move it to a drive that keeps folders private. to keep it on this drive, choose that on your journal's health page.

journal.cli.doctor.backlog.pending = pending: { $days }

journal.cli.doctor.backlog.stuck = stuck: { $days }

journal.cli.doctor.backlog.unknown = unknown: { $days }

journal.cli.doctor.device.refusal.count = { $quantity ->
    [one] device { $device } had a request turned away: { $reason }, { $attempts }x through { $last_seen }
   *[other] device { $device } had requests turned away: { $reason }, { $attempts }x through { $last_seen }
}

journal.cli.doctor.facet.fix = { $unrouted_quantity ->
    [one] that segment is missing from the activity lists; check the facet declarations with `{ $doctor_command }` and recent sense runs with `{ $logs_command }`
   *[other] those segments are missing from the activity lists; check the facet declarations with `{ $doctor_command }` and recent sense runs with `{ $logs_command }`
}

journal.cli.doctor.facet.no.active = no active segments in the newest days

journal.cli.doctor.facet.routed = { $active_quantity ->
    [one] { $routed } of { $active } active segment since { $since } filed under a facet
   *[other] { $routed } of { $active } active segments since { $since } filed under a facet
}

journal.cli.doctor.facet.unrouted = { $active_quantity ->
    [one] { $unrouted } of { $active } active segment since { $since } { $unrouted_quantity ->
        [one] was
       *[other] were
    } filed under no facet
   *[other] { $unrouted } of { $active } active segments since { $since } { $unrouted_quantity ->
        [one] was
       *[other] were
    } filed under no facet
}

journal.cli.doctor.orphan.pdf.found = { $quantity ->
    [one] { $count } raw PDF original without a readable document transcript: { $paths }
   *[other] { $count } raw PDF original(s) without a readable document transcript: { $paths }
}

journal.cli.doctor.service.crash.list = crash-loop: { $services }

journal.cli.doctor.service.restart.attempts = { $quantity ->
    [one] { $name } ({ $attempts } restart attempt)
   *[other] { $name } ({ $attempts } restart attempts)
}

journal.cli.doctor.status.error = ERROR

journal.cli.doctor.status.fail = FAIL

journal.cli.doctor.status.ok = OK

journal.cli.doctor.status.skip = SKIP

journal.cli.doctor.status.warn = WARN

journal.cli.doctor.summary = { $total_quantity ->
    [one] doctor: { $total } check, { $failed } failed, { $warnings } { $warning_quantity ->
        [one] warning
       *[other] warnings
    }, { $skipped } skipped, { $errors } { $error_quantity ->
        [one] error
       *[other] errors
    }
   *[other] doctor: { $total } checks, { $failed } failed, { $warnings } { $warning_quantity ->
        [one] warning
       *[other] warnings
    }, { $skipped } skipped, { $errors } { $error_quantity ->
        [one] error
       *[other] errors
    }
}

journal.cli.doctor.transcription.unretryable = { $quantity ->
    [one] { $count } recording the journal will not retry on its own ({ $handler_key }: { $handler }, { $reason_key }: { $reason_code }): { $identities }
   *[other] { $count } recordings the journal will not retry on its own ({ $handler_key }: { $handler }, { $reason_key }: { $reason_code }): { $identities }
}

journal.cli.doctor.check.deferred = deferred check set: { $set }

journal.cli.doctor.execution.error = check execution failed: { $kind }

journal.cli.doctor.execution.error.detail = check execution failed: { $kind }: { $error }

journal.cli.doctor.platform.unsupported = not supported on { $platform }

journal.cli.doctor.progress.check = { $seconds }s checking { $name }

journal.cli.doctor.progress.hash = { $seconds }s verifying { $member }: { $hashed } of { $total } { $megabytes }

journal.cli.doctor.service.conflict.warning = resolve the macOS supervisor topology warning before changing the journal service

journal.cli.doctor.supervisor.conflict.repair = { $quantity ->
    [one] remove the foreign launcher targeting { $app_path }: { $commands }; then rerun { $doctor_command }
   *[other] remove foreign launchers targeting { $app_path }: { $commands }; then rerun { $doctor_command }
}

journal.cli.doctor.supervisor.conflict.report = { $quantity ->
    [one] macOS supervisor conflict: { $count } foreign { $keep_alive } launcher targets { $app_path }
   *[other] macOS supervisor conflict: { $count } foreign { $keep_alive } launchers target { $app_path }
}

journal.cli.doctor.supervisor.conflict.resolve = { $quantity ->
    [one] resolve the macOS supervisor conflict first: remove the foreign launcher targeting { $app_path }: { $commands }; then rerun { $doctor_command }
   *[other] resolve the macOS supervisor conflict first: remove foreign launchers targeting { $app_path }: { $commands }; then rerun { $doctor_command }
}

journal.cli.doctor.parakeet.unreachable = { $cause ->
    [absent] { $server } not reachable: nothing is listening
    [unknown] { $server } not reachable: can't tell whether anything is listening
    [runtime] { $server } not reachable: nothing answered
    [timeout] { $server } not reachable: took too long to answer
   *[transport] { $server } not reachable: the connection dropped
}

journal.cli.doctor.sense.dispatch.capped.reason = at least 99 Sense dispatch errors since the last successful handler sync; latest reason: { $reason }

journal.cli.doctor.sense.dispatch.capped.unknown = at least 99 Sense dispatch errors since the last successful handler sync; latest reason unavailable

journal.cli.doctor.sense.dispatch.errors.reason = { $quantity ->
    [one] { $count } Sense dispatch error since the last successful handler sync; latest reason: { $reason }
   *[other] { $count } Sense dispatch errors since the last successful handler sync; latest reason: { $reason }
}

journal.cli.doctor.sense.dispatch.errors.unknown = { $quantity ->
    [one] { $count } Sense dispatch error since the last successful handler sync; latest reason unavailable
   *[other] { $count } Sense dispatch errors since the last successful handler sync; latest reason unavailable
}

journal.cli.doctor.sense.dispatch.unreachable = { $cause ->
    [absent] could not confirm Sense dispatch health: nothing is listening
    [unknown] could not confirm Sense dispatch health: can't tell whether anything is listening
    [runtime] could not confirm Sense dispatch health: nothing answered
    [timeout] could not confirm Sense dispatch health: took too long to answer
   *[transport] could not confirm Sense dispatch health: the connection dropped
}

journal.cli.doctor.service.running.unreachable = { $cause ->
    [absent] couldn't get a status — nothing is listening
    [unknown] couldn't get a status — can't tell whether anything is listening
    [runtime] couldn't get a status — nothing answered
    [timeout] couldn't get a status — took too long to answer
   *[transport] couldn't get a status — the connection dropped
}

journal.cli.doctor.sources.days.more = { $days }, and { $more } more

journal.cli.doctor.sources.fold.failed = { $quantity ->
    [one] { $count } day didn't finish processing ({ $days })
   *[other] { $count } day(s) didn't finish processing ({ $days })
}

journal.cli.doctor.sources.statistics.refresh = run { $command }; check the health logs if it persists

journal.cli.doctor.sources.statistics.stale = couldn't check — the statistics are { $hours } hours old

journal.cli.doctor.sources.unreadable = { $quantity ->
    [one] { $count } day couldn't be scanned ({ $days })
   *[other] { $count } day(s) couldn't be scanned ({ $days })
}

journal.cli.doctor.sources.unreadable.cause = { $quantity ->
    [one] { $count } day couldn't be scanned ({ $days }): { $day }: { $cause }
   *[other] { $count } day(s) couldn't be scanned ({ $days }): { $day }: { $cause }
}

journal.cli.doctor.sources.unreadable.undated = { $quantity ->
    [one] { $count } day couldn't be scanned ({ $days }): that day: { $cause }
   *[other] { $count } day(s) couldn't be scanned ({ $days }): that day: { $cause }
}

journal.cli.doctor.backlog.band = { $recording_count ->
    [one] band { $band }: { $recordings } recording ready / { $recordings_left } left, { $outputs } daily { $output_count ->
        [one] output
       *[other] outputs
    } ready / { $outputs_left } left, { $activities } { $activity_count ->
        [one] activity
       *[other] activities
    } left, { $requests } owner { $request_count ->
        [one] request
       *[other] requests
    }, { $permission } waiting for permission, { $prerequisites } waiting for prerequisites
   *[other] band { $band }: { $recordings } recordings ready / { $recordings_left } left, { $outputs } daily { $output_count ->
        [one] output
       *[other] outputs
    } ready / { $outputs_left } left, { $activities } { $activity_count ->
        [one] activity
       *[other] activities
    } left, { $requests } owner { $request_count ->
        [one] request
       *[other] requests
    }, { $permission } waiting for permission, { $prerequisites } waiting for prerequisites
}

journal.cli.doctor.backlog.caught = caught up

journal.cli.doctor.backlog.caught.capped = { $day_count ->
    [one] caught up; { $days } day completed with { $unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
   *[other] caught up; { $days } day(s) completed with { $unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
}

journal.cli.doctor.backlog.change.recorded = { $name } recorded a change on { $location }; resolve the recorded change before that day is redone

journal.cli.doctor.backlog.change.started = { $name } may have started a change but did not confirm completion on { $location }; resolve the in-progress change before reprocessing

journal.cli.doctor.backlog.checked = recent work checked

journal.cli.doctor.backlog.checked.capped = { $day_count ->
    [one] recent work checked; { $days } day completed with { $unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
   *[other] recent work checked; { $days } day(s) completed with { $unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
}

journal.cli.doctor.backlog.deferred = { $name } stopped on { $location }; it can retry on a later run after the local day changes

journal.cli.doctor.backlog.deferred.reason = { $name } stopped on { $location } with reason { $reason }; it can retry on a later run after the local day changes

journal.cli.doctor.backlog.eligible = { $name } stopped on { $location }; it can retry on the next eligible run

journal.cli.doctor.backlog.eligible.reason = { $name } stopped on { $location } with reason { $reason }; it can retry on the next eligible run

journal.cli.doctor.backlog.empty.date = none

journal.cli.doctor.backlog.exhausted = { $name } stopped on { $location } after its automatic retry; run { $command }

journal.cli.doctor.backlog.exhausted.reason = { $name } stopped on { $location } with reason { $reason } after its automatic retry; run { $command }

journal.cli.doctor.backlog.hold.retry = { $day }: { $reason } (retry { $unit })

journal.cli.doctor.backlog.inventory.partial.current = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; set aside: { $holds }
}

journal.cli.doctor.backlog.inventory.partial.last = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; last known progress; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; last known progress; set aside: { $holds }
}

journal.cli.doctor.backlog.inventory.whole.current = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; set aside: { $holds }
}

journal.cli.doctor.backlog.inventory.whole.last = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; last known progress; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; last known progress; set aside: { $holds }
}

journal.cli.doctor.backlog.pending.counts = { $pending_count ->
    [one] { $pending } day pending, { $stuck } { $stuck_count ->
        [one] day
       *[other] day(s)
    } stuck
   *[other] { $pending } day(s) pending, { $stuck } { $stuck_count ->
        [one] day
       *[other] day(s)
    } stuck
}

journal.cli.doctor.backlog.pending.oldest = { $pending_count ->
    [one] { $pending } day pending, { $stuck } { $stuck_count ->
        [one] day
       *[other] day(s)
    } stuck; oldest outstanding { $day }
   *[other] { $pending } day(s) pending, { $stuck } { $stuck_count ->
        [one] day
       *[other] day(s)
    } stuck; oldest outstanding { $day }
}

journal.cli.doctor.backlog.resolve = { $name } stopped on { $location }; resolve the conflict before reprocessing a past day

journal.cli.doctor.backlog.resolve.reason = { $name } stopped on { $location } with reason { $reason }; resolve the conflict before reprocessing a past day

journal.cli.doctor.backlog.retry = { $name } stopped on { $location }; run { $command }

journal.cli.doctor.backlog.retry.reason = { $name } stopped on { $location } with reason { $reason }; run { $command }

journal.cli.doctor.backlog.review.started = { $name } may have started an entity review change but did not confirm completion on { $location }; resolve the in-progress change before reprocessing

journal.cli.doctor.backlog.unfinished = { $activity_count ->
    [one] { $activities } activity on { $days } completed { $day_count ->
        [one] day
       *[other] days
    } couldn't finish processing
   *[other] { $activities } activities on { $days } completed { $day_count ->
        [one] day
       *[other] days
    } couldn't finish processing
}

journal.cli.doctor.backlog.unknown.count = { $quantity ->
    [one] couldn't fully determine; { $days } day unknown
   *[other] couldn't fully determine; { $days } day(s) unknown
}

journal.cli.doctor.backup.association = { $association ->
    [unbound] setup association unknown; history only
    [unknown] current setup association unavailable
    [superseded] superseded setup; history only
    [current] matches the selected setup
   *[invalid] recorded setup association unavailable
}

journal.cli.doctor.backup.attempt.incomplete = { $outcome ->
    [success] backup on; latest recorded attempt: successful copy at { $date }; some files were not copied; { $history }; { $association }
    [incomplete] backup on; latest recorded attempt: incomplete copy at { $date }; some files were not copied; { $history }; { $association }
   *[failed] backup on; latest recorded attempt: failed at { $date }; some files were not copied; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.locked = { $outcome ->
    [success] backup on; latest recorded attempt: successful copy at { $date }; repository was locked; { $history }; { $association }
    [incomplete] backup on; latest recorded attempt: incomplete copy at { $date }; repository was locked; { $history }; { $association }
   *[failed] backup on; latest recorded attempt: failed at { $date }; repository was locked; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.ordinary = { $outcome ->
    [success] backup on; latest recorded attempt: successful copy at { $date }; { $history }; { $association }
    [incomplete] backup on; latest recorded attempt: incomplete copy at { $date }; { $history }; { $association }
   *[failed] backup on; latest recorded attempt: failed at { $date }; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.refused = { $outcome ->
    [success] backup on; latest recorded attempt: successful copy at { $date }; backup key refused; { $history }; { $association }
    [incomplete] backup on; latest recorded attempt: incomplete copy at { $date }; backup key refused; { $history }; { $association }
   *[failed] backup on; latest recorded attempt: failed at { $date }; backup key refused; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.timeout = { $outcome ->
    [success] backup on; latest recorded attempt: successful copy at { $date }; timed out; { $history }; { $association }
    [incomplete] backup on; latest recorded attempt: incomplete copy at { $date }; timed out; { $history }; { $association }
   *[failed] backup on; latest recorded attempt: failed at { $date }; timed out; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.unknown = { $outcome ->
    [success] backup on; latest recorded attempt: successful copy at { $date }; cause unknown; { $history }; { $association }
    [incomplete] backup on; latest recorded attempt: incomplete copy at { $date }; cause unknown; { $history }; { $association }
   *[failed] backup on; latest recorded attempt: failed at { $date }; cause unknown; { $history }; { $association }
}

journal.cli.doctor.backup.date.unknown = unknown date

journal.cli.doctor.backup.details = open backup or health in your journal for details

journal.cli.doctor.backup.history.incomplete.dated = { $quantity ->
    [one] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } second; incomplete copy, some files were not copied; present coverage and verification unknown
   *[other] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } seconds; incomplete copy, some files were not copied; present coverage and verification unknown
}

journal.cli.doctor.backup.history.incomplete.undated = last successful copy (historical): snapshot { $snapshot } at { $date }, age unknown; incomplete copy, some files were not copied; present coverage and verification unknown

journal.cli.doctor.backup.history.success.dated = { $quantity ->
    [one] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } second; present coverage and verification unknown
   *[other] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } seconds; present coverage and verification unknown
}

journal.cli.doctor.backup.history.success.undated = last successful copy (historical): snapshot { $snapshot } at { $date }, age unknown; present coverage and verification unknown

journal.cli.doctor.backup.history.unknown = last successful copy unknown

journal.cli.doctor.backup.invalid.history = backup on; recorded status unavailable; { $history }

journal.cli.doctor.backup.paused.history = backup paused by you since { $date }; { $history }

journal.cli.doctor.backup.snapshot.unknown = unknown

journal.cli.doctor.durability.action.recovered = no action is needed: recovered records stay beside the originals and are listed here

journal.cli.doctor.durability.action.refused = the journal will not start until this clears; each line above says whether it clears after the computer restarts or needs help

journal.cli.doctor.durability.action.unreadable = check the paths under 'could not inspect'; any recovered records stay beside the originals and are listed here

journal.cli.doctor.durability.admission.helper = { $finding ->
    [exited] generation { $generation } admission { $admission }: helper { $identity }, exited
    [escalated] generation { $generation } admission { $admission }: helper { $identity }, retired (escalated: true)
    [retired] generation { $generation } admission { $admission }: helper { $identity }, retired
    [unacknowledged] generation { $generation } admission { $admission }: helper { $identity }, never-acknowledged
    [spawn] generation { $generation } admission { $admission }: helper { $identity }, spawn-failed
    [unidentified] generation { $generation } admission { $admission }: helper { $identity }, unidentified
    [rejected] generation { $generation } admission { $admission }: helper { $identity }, rejected-and-reaped
   *[reused] generation { $generation } admission { $admission }: helper { $identity }, pid-reused-by-another-user
}

journal.cli.doctor.durability.admission.service = { $finding ->
    [exited] generation { $generation } admission { $admission }: service { $identity }, exited
    [escalated] generation { $generation } admission { $admission }: service { $identity }, retired (escalated: true)
    [retired] generation { $generation } admission { $admission }: service { $identity }, retired
    [unacknowledged] generation { $generation } admission { $admission }: service { $identity }, never-acknowledged
    [spawn] generation { $generation } admission { $admission }: service { $identity }, spawn-failed
    [unidentified] generation { $generation } admission { $admission }: service { $identity }, unidentified
    [rejected] generation { $generation } admission { $admission }: service { $identity }, rejected-and-reaped
   *[reused] generation { $generation } admission { $admission }: service { $identity }, pid-reused-by-another-user
}

journal.cli.doctor.durability.clean = no recorded recovery to report

journal.cli.doctor.durability.clean.outstanding = { $quantity ->
    [one] no recorded recovery to report; the newest run (generation { $generation }) still owes { $launches } launch
   *[other] no recorded recovery to report; the newest run (generation { $generation }) still owes { $launches } launches
}

journal.cli.doctor.durability.closed = { $quantity ->
    [one] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admission, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted
   *[other] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admissions, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted
}

journal.cli.doctor.durability.closed.restarted = { $quantity ->
    [one] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admission, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted; the computer had restarted since
   *[other] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admissions, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted; the computer had restarted since
}

journal.cli.doctor.durability.heading.aside = bookkeeping set aside ({ $count }):

journal.cli.doctor.durability.heading.closed = runs closed by a later start ({ $count }):

journal.cli.doctor.durability.heading.refused = starts refused ({ $count }):

journal.cli.doctor.durability.heading.unreadable = could not inspect ({ $count }):

journal.cli.doctor.durability.heading.unresolved = runs that did not stop cleanly ({ $count }):

journal.cli.doctor.durability.more = ... and { $count } more

journal.cli.doctor.durability.outstanding = { $quantity ->
    [one] the newest run (generation { $generation }) still owes { $launches } launch
   *[other] the newest run (generation { $generation }) still owes { $launches } launches
}

journal.cli.doctor.durability.refusal.restart = generation { $generation }: the record of processes a retirement started is lost, so the journal will not start; it clears after the computer restarts

journal.cli.doctor.durability.refusal.support = generation { $generation }: the record of processes a retirement started is lost, so the journal will not start; it will not clear on its own and needs help from support

journal.cli.doctor.durability.unresolved = generation { $generation }: { $reason }

journal.cli.doctor.capture.recent = the { $brand } app on { $client } last added { $hours }h ago

journal.cli.doctor.capture.recent.sources = { $quantity ->
    [one] the { $brand } app on { $client } last added { $hours }h ago ({ $sources } needs attention)
   *[other] the { $brand } app on { $client } last added { $hours }h ago ({ $sources } need attention)
}

journal.cli.doctor.capture.rollup = { $rollup }; { $devices }

journal.cli.doctor.capture.sources = the { $brand } app on { $client } is having trouble adding { $sources }

journal.cli.doctor.capture.trouble = the { $brand } app on { $client } is having trouble adding

journal.cli.doctor.capture.trouble.sources = { $quantity ->
    [one] the { $brand } app on { $client } is having trouble adding ({ $sources } needs attention)
   *[other] the { $brand } app on { $client } is having trouble adding ({ $sources } need attention)
}

journal.cli.doctor.clients.ledger.unavailable = authorized client ledger unavailable: { $reason }

journal.cli.doctor.delivery.rejected = the journal rejected an upload from { $client }

journal.cli.doctor.delivery.rejected.sources = { $quantity ->
    [one] the journal rejected an upload from { $client } ({ $sources } needs attention)
   *[other] the journal rejected an upload from { $client } ({ $sources } need attention)
}

journal.cli.doctor.devices.more = { $devices }{ $separator }+{ $extra } more

journal.cli.doctor.executable.directory = could not determine executable directory: { $path }

journal.cli.doctor.parakeet.phase = { $server } not reachable: your journal reports its state as { $phase }

journal.cli.doctor.parakeet.phase.checking = { $server } not reachable: your journal reports its state as checking

journal.cli.doctor.parakeet.reinstall = reinstall the journal

journal.cli.doctor.source.default = default

journal.cli.doctor.speakers.ready = { $command } installation ready

journal.cli.doctor.speakers.unavailable = { $command } installation unavailable

journal.cli.doctor.sync.local = this computer only ({ $host })

journal.cli.doctor.sync.peer.legacy = this computer only ({ $host }){ $newline }  last other writer: { $writer } (legacy machine { $prefix }...)

journal.cli.doctor.sync.peer.unknown = this computer only ({ $host }){ $newline }  last other writer: { $writer } (unidentified heartbeat)

journal.cli.doctor.sync.peer.writer = this computer only ({ $host }){ $newline }  last other writer: { $writer } (writer { $prefix }... run { $run })

journal.cli.doctor.tasks.duration = { $task } ({ $seconds }s of { $limit }s cap)

journal.cli.doctor.tasks.long = running long: { $tasks }

journal.cli.doctor.holds.partition = held partition { $partition }

journal.cli.doctor.holds.partition.stopped = partition { $partition } was running when the journal stopped; the journal holds it at its next start until its processes are gone

journal.cli.doctor.holds.record.empty = record file is empty

journal.cli.doctor.holds.record.filename = invalid partition file name in scope directory; the journal still reads it as a task record

journal.cli.doctor.holds.record.invalid = cannot parse record: { $error }

journal.cli.doctor.holds.record.stray = non-record entry in scope directory

journal.cli.doctor.holds.record.unreadable = cannot read record: { $error }

journal.cli.doctor.holds.remove = remove { $path }: it holds nothing

journal.cli.doctor.holds.scope.unknown = unrecognized scope directory format; the journal still holds any task recorded in it

journal.cli.doctor.holds.scope.unreadable = cannot list scope directory: { $error }

journal.cli.doctor.holds.stray = stray non-directory in task-queue in-flight path

journal.cli.doctor.holds.unreadable = cannot list in-flight holds directory: { $error }

journal.cli.doctor.parakeet.coreml.configured = parakeet check failed: installed cache { $installed } does not match configured { $configured }

journal.cli.doctor.parakeet.coreml.digest = parakeet check failed: asset digest mismatch for { $file }: expected { $expected }, found { $found }

journal.cli.doctor.parakeet.coreml.failed = parakeet check failed: { $error }

journal.cli.doctor.parakeet.coreml.missing = parakeet check failed: asset missing: { $file }

journal.cli.doctor.parakeet.coreml.missing.error = parakeet check failed: asset missing: { $file }: { $error }

journal.cli.doctor.parakeet.coreml.sentinel = parakeet check failed: sentinel not ready at { $path }

journal.cli.doctor.parakeet.coreml.size = parakeet check failed: asset size mismatch for { $file }: expected { $expected }, found { $found }

journal.cli.doctor.parakeet.coreml.unreadable = parakeet check failed: failed to read { $file }: { $error }

journal.cli.doctor.parakeet.cpp.executable = { $parakeet_cpp } check failed: { $name } not executable at { $path }

journal.cli.doctor.parakeet.cpp.missing = { $parakeet_cpp } check failed: { $name } missing at { $path }

journal.cli.doctor.parakeet.cpp.model = { $parakeet_cpp } check failed: model missing at { $path }

journal.cli.doctor.parakeet.cpp.pin = { $parakeet_cpp } pin lacks { $key }

journal.cli.doctor.parakeet.cpp.platform = { $parakeet_cpp } is unsupported on { $os }/{ $arch }

journal.cli.doctor.vad.contract = { $vad } helper ran but did not report closed-stdin usage contract (exit { $exit_code }): { $stderr }

journal.cli.doctor.vad.loader = { $vad } helper could not load { $library }

journal.cli.doctor.vad.missing = { $vad } helper binary is missing: { $path }

journal.cli.doctor.vad.ready = { $vad } helper launchable (closed-stdin usage contract)

journal.cli.doctor.vad.repair.install = reinstall the journal, then rerun { $doctor_command }

journal.cli.doctor.vad.repair.install.app = reinstall the journal app, then rerun { $doctor_command }

journal.cli.doctor.vad.repair.restart = restart the journal, then rerun { $doctor_command }; if it still times out, reinstall the journal

journal.cli.doctor.vad.repair.restart.app = restart the journal app, then rerun { $doctor_command }; if it still times out, reinstall the journal app

journal.cli.doctor.vad.spawn = { $vad } helper could not be executed: { $cause }

journal.cli.doctor.vad.timeout = { $vad } helper did not exit within { $timeout }

journal.cli.doctor.vad.unresolved = could not resolve the { $vad } helper path: { $cause }

journal.cli.doctor.sync.directory.required = flat-directory descendant is not a directory: { $path }

journal.cli.doctor.sync.entry.changed = entry changed during observation: { $reason }

journal.cli.doctor.sync.entry.invalid = invalid flat-directory entry { $entry }: { $reason }

journal.cli.doctor.sync.entry.replaced = changed or disappeared during inspection

journal.cli.doctor.sync.entry.unsafe = Installation: needs attention{ $newline }your journal contains an item that can't be checked safely.{ $newline }move the item named under details out of your journal's `{ $sync_folder }` folder, then try again.{ $newline }{ $newline }details:{ $newline }path: { $path }{ $newline }type: { $entry_type }{ $newline }reason: { $reason }

journal.cli.doctor.sync.entry.unsafe.bytes = Installation: needs attention{ $newline }your journal contains an item that can't be checked safely.{ $newline }move the item named under details out of your journal's `{ $sync_folder }` folder, then try again.{ $newline }{ $newline }details:{ $newline }path: { $path }{ $newline }type: { $entry_type }{ $newline }reason: { $reason }{ $newline }name bytes: { $bytes }

journal.cli.doctor.sync.entry.vanished = flat-directory entry vanished while listing: { $path }

journal.cli.doctor.sync.file.required = flat-directory entry is not a regular file: { $path }

journal.cli.doctor.sync.folder.full = Installation: needs attention{ $newline }your journal has too many items to check safely.{ $newline }review the items in your journal's `{ $sync_folder }` folder. move only items you know do not belong there until 256 or fewer remain, then try again.{ $newline }{ $newline }details:{ $newline }folder: { $folder }{ $newline }items found: more than 256{ $newline }maximum checked: 256

journal.cli.doctor.sync.folder.unsafe = Installation: needs attention{ $newline }part of your journal can't be checked safely.{ $newline }check the path named under details. it needs to be an ordinary folder inside your journal that you can open before you try again.{ $newline }{ $newline }details:{ $newline }path: { $path }{ $newline }reason: { $reason }

journal.cli.doctor.sync.heartbeat = Installation: needs attention{ $newline }a recent heartbeat from another run is present.{ $newline }wait a moment, then try again.

journal.cli.doctor.sync.identity.changed = flat-directory identity changed: { $path }

journal.cli.doctor.sync.io = { $operation } failed for { $path }: { $reason }

journal.cli.doctor.sync.kind.blockdevice = block device

journal.cli.doctor.sync.kind.characterdevice = character device

journal.cli.doctor.sync.kind.directory = directory

journal.cli.doctor.sync.kind.fifo = named pipe

journal.cli.doctor.sync.kind.other = other filesystem object

journal.cli.doctor.sync.kind.regularfile = regular file

journal.cli.doctor.sync.kind.socket = Unix socket

journal.cli.doctor.sync.kind.symlink = symbolic link

journal.cli.doctor.sync.path.invalid = invalid flat-directory path { $path }: { $reason }

journal.cli.doctor.sync.reason.disappeared = entry disappeared during observation

journal.cli.doctor.sync.reason.nonregular = not a regular file ({ $kind })

journal.cli.doctor.sync.reason.oversized = regular file is { $size } bytes, limit is { $limit }

journal.cli.doctor.sync.reason.read = could not read observed file

journal.cli.doctor.sync.size = flat-directory entry exceeds observed-read limit: { $path } is { $kind }, { $size } bytes exceeds { $limit }

journal.cli.doctor.sync.symlink = flat-directory descendant is a symlink: { $path }

journal.cli.doctor.sync.unsupported = Installation: needs attention{ $newline }sync rescan capability is unavailable on this platform.

journal.cli.doctor.sync.unverifiable = Installation: needs attention{ $newline }startup status couldn't be verified.{ $newline }wait a moment, then try again.

journal.cli.doctor.sync.waiting = Installation: waiting{ $newline }a recent heartbeat from another run was found.{ $newline }the { $brand } app is waiting while it checks whether that activity continues. it will check again shortly.

journal.cli.doctor.package.executable.parent = executable has no parent

journal.cli.doctor.package.layout = could not locate packaged talent roots from executable directory { $directory }{ $newline }walked ancestor { $ancestor }{ $newline }site-packages candidate: { $site }{ $newline }checkout candidate ({ $pyproject } + { $git_dir }): { $checkout }{ $newline }layout share anchors { $bundle }, { $layout }, { $template }: { $install }{ $newline }Apple bundle { $journal_payload } candidate: { $embedded }

journal.cli.doctor.package.missing = installation root { $root } is missing directory { $path }

journal.cli.doctor.package.none = none

journal.cli.doctor.package.target = { $newline }Cargo target directory { $target } is outside any checkout: a binary built there cannot find the payload, so keep { $target_env } inside the checkout

journal.cli.doctor.package.unavailable = could not locate packaged talent roots from executable directory { $directory }

journal.cli.doctor.speakers.installation.app = { $speakers_name } installation is incomplete ({ $detail }). Repair: reinstall the journal app.

journal.cli.doctor.speakers.installation.journal = { $speakers_name } installation is incomplete ({ $detail }). Repair: reinstall the journal.

journal.cli.doctor.speakers.reinstall.app = reinstall the journal app

journal.cli.doctor.windows.parakeet.executable = could not determine the running journal executable: { $error }

journal.cli.doctor.windows.parakeet.layout = running journal executable is not in the package bin directory: { $path }

journal.cli.doctor.windows.parakeet.member = signed { $parakeet_name } app payload refuses { $member }: { $error }

journal.cli.doctor.windows.parakeet.parent = running journal executable has no containing directory: { $path }

journal.cli.doctor.windows.parakeet.platform = Windows { $parakeet_name } package verification requires a Windows runtime

journal.cli.doctor.windows.parakeet.root = package bin directory has no package root: { $path }

journal.cli.doctor.windows.parakeet.verification = could not verify the signed { $parakeet_name } app payload: { $error }

journal.cli.doctor.storage.inspection.log = storage privacy: { $error }

journal.cli.doctor.windows.onnx.member = signed { $onnx } app payload refuses { $member }: { $error }

journal.cli.doctor.windows.onnx.platform = { $windows } { $onnx } package verification requires a { $windows } runtime

journal.cli.doctor.windows.onnx.verification = could not verify the signed { $onnx } app payload: { $error }

journal.cli.doctor.vad.executable = could not determine current executable for { $vad } helper: { $error }

journal.cli.doctor.vad.layout = { $vad } helper lookup is outside the running signed app payload

journal.cli.doctor.vad.parent = current executable has no parent directory for { $vad } helper lookup

journal.cli.doctor.vad.probe.log = { $vad } helper probe failed: { $error }

journal.cli.doctor.windows.onnx.helper = { $onnx } helper is not the declared member of the signed app payload

journal.cli.doctor.search.log.finished = last finished search check could not be read: { $error }

journal.cli.doctor.search.log.unavailable = search index status unavailable: { $error }

journal.cli.doctor.search.log.work = search work list could not be read: { $error }

journal.cli.about.os.release.unavailable = about: { $release_file } unavailable: { $error }

journal.cli.about.resource.unavailable = the journal's about information couldn't be read. check that your journal is running, then try again.

journal.cli.about.usage.journal = usage: { $syntax }

journal.cli.about.usage.remote = usage: { $syntax }

journal.cli.client.error.malformed = the journal response couldn't be read.

journal.cli.client.error.timeout = The journal didn't answer in time.

journal.cli.client.error.unreachable = your journal couldn't be reached over { $http }.

journal.cli.client.error.unreadable = The journal returned an unreadable error.

journal.cli.client.port.invalid = { $variable } must be an integer from 1 to 65535.

journal.cli.describe.usage.argument.unknown = usage: { $syntax }{ $line_break }{ $command }: error: unknown argument: { $argument }

journal.cli.describe.usage.backfill.arguments = usage: { $syntax }{ $line_break }{ $command }: error: face backfill requires { $arguments }

journal.cli.describe.usage.backfill.budget = usage: { $syntax }{ $line_break }{ $command }: error: face backfill budget is outside the allowed range

journal.cli.describe.usage.backfill.desktop = usage: { $syntax }{ $line_break }{ $command }: error: face backfill is available on desktop journals only

journal.cli.describe.usage.describe.repeated = usage: { $syntax }{ $line_break }{ $command }: error: { $describe_flag } was provided more than once

journal.cli.describe.usage.file.extra = usage: { $syntax }{ $line_break }{ $command }: error: only one video path may be provided

journal.cli.describe.usage.file.required = usage: { $syntax }{ $line_break }{ $command }: error: the following arguments are required: { $file }

journal.cli.describe.usage.frames.repeated = usage: { $syntax }{ $line_break }{ $command }: error: { $frames_only_flag } was provided more than once

journal.cli.describe.usage.help = usage: { $syntax }

journal.cli.describe.usage.help.extra = usage: { $syntax }{ $line_break }{ $command }: error: { $help_flag } does not accept other arguments

journal.cli.describe.usage.jobs.positive = usage: { $syntax }{ $line_break }{ $command }: error: { $jobs_flag } requires a positive integer

journal.cli.describe.usage.jobs.repeated = usage: { $syntax }{ $line_break }{ $command }: error: { $jobs_flag } was provided more than once

journal.cli.describe.usage.journal.path = usage: { $syntax }{ $line_break }{ $command }: error: { $journal_flag } requires a path

journal.cli.describe.usage.journal.repeated = usage: { $syntax }{ $line_break }{ $command }: error: { $journal_flag } was provided more than once

journal.cli.describe.usage.mode.required = usage: { $syntax }{ $line_break }{ $command }: error: exactly one of { $frames_only_flag } or { $describe_flag } is required

journal.cli.describe.usage.redo.mode = usage: { $syntax }{ $line_break }{ $command }: error: { $redo_flag } requires { $describe_flag }

journal.cli.describe.usage.redo.repeated = usage: { $syntax }{ $line_break }{ $command }: error: { $redo_flag } was provided more than once

journal.cli.describe.usage.registry.extra = usage: { $syntax }{ $line_break }{ $command }: error: { $category_registry_flag } does not accept other arguments

journal.cli.describe.usage.version.extra = usage: { $syntax }{ $line_break }{ $command }: error: { $version_flag } does not accept other arguments

journal.cli.stats.arguments = usage: { $syntax }{ $line_break }{ $command }: error: unrecognized arguments: { $argument }

journal.cli.stats.cause.facet = facet store error: { $detail }

journal.cli.stats.cause.invalid.day = invalid day { $day }; expected { $date_format } calendar date

journal.cli.stats.cause.io = cannot access { $path }: { $detail }

journal.cli.stats.cause.path = journal path error: { $detail }

journal.cli.stats.cause.read = journal read error: { $detail }

journal.cli.stats.cause.schema = stats validation failed: { $schema } is { $actual }, expected { $expected }

journal.cli.stats.cause.timestamp = stats validation failed: { $timestamp } must be non-empty

journal.cli.stats.cause.validation = stats validation failed: { $detail }

journal.cli.stats.cause.write = journal atomic-write error: { $detail }

journal.cli.stats.configuration = Error loading the journal's daily configuration: { $error }

journal.cli.stats.day.cache = Day cache save failed for { $day }: { $error }

journal.cli.stats.enumerate = Error enumerating journal days: { $error }

journal.cli.stats.help = usage: { $syntax }{ $line_break }{ $line_break }Scan a { $brand } journal and generate statistics{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_5 }show this help message and exit{ $line_break }{ $indent_2 }{ $no_cache_option }{ $gap_5 }Disable per-day caching (force re-scan all days){ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_2 }Enable verbose output{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_4 }Enable debug logging

journal.cli.stats.publication.durable = published; durability uncertain: { $source }

journal.cli.stats.publication.observed = published; observation uncertain: { $source }

journal.cli.stats.roots = journal-stats failed: { $error }

journal.cli.stats.saved = Statistics saved to { $path }

journal.cli.stats.tokens.directory = Token directory read failed: { $path }

journal.cli.stats.tokens.json = Invalid token { $json_format } in { $path }: { $error }

journal.cli.stats.tokens.load = Token cache load failed for { $path }: { $error }

journal.cli.stats.tokens.read = Token read failed for { $path }: { $error }

journal.cli.stats.tokens.save = Token cache save failed for { $path }: { $error }

journal.cli.stats.validate = Error validating { $filename }: { $error }

journal.cli.stats.write = Error writing { $filename }: { $error }

journal.cli.stats.written = { $days ->
    [one] Wrote stats for { $days } day to { $path }
   *[other] Wrote stats for { $days } day(s) to { $path }
}

journal.cli.path.home = journal-path failed: could not determine home directory

journal.cli.path.utf8 = journal-path failed: config is not valid { $encoding }

journal.cli.stats.overrides = stats validation failed: failed to read talent overrides from { $path }

journal.cli.path.create = could not create journal directory ({ $source }): { $path }: { $error }

journal.cli.backfill.committed = COMMITTED

journal.cli.backfill.day.missing = Day { $day } was not found in the journal

journal.cli.backfill.days = Could not list chronicle days: { $error }

journal.cli.backfill.flags.conflict = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: error: argument { $flag }: not allowed with argument { $previous }

journal.cli.backfill.flags.missing = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: error: argument { $day_flag }: expected one argument

journal.cli.backfill.flags.unknown = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: error: unrecognized arguments: { $argument }

journal.cli.backfill.help = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $line_break }Backfill empty processing records onto stuck header-only native{ $line_break }{ $handlers } outputs{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_5 }show this help message and exit{ $line_break }{ $indent_2 }{ $day_option }{ $gap_6 }Day in { $date_format } format; defaults to all days{ $line_break }{ $indent_2 }{ $commit_option }{ $gap_7 }Rewrite eligible { $jsonl_format } headers with empty processing records{ $line_break }{ $indent_2 }{ $dry_run_option }{ $gap_6 }Preview eligible outputs without writing changes{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_2 }Enable verbose output{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_4 }Enable debug logging

journal.cli.backfill.identity.ambiguous = named stream directory "{ $default_stream }" cannot be spelled as a record identity: { $path }

journal.cli.backfill.identity.duplicate = multiple segments share stream { $stream } key { $key }

journal.cli.backfill.identity.encoding = segment path is not { $encoding } representable: { $path }

journal.cli.backfill.invalid.day = expected day in { $date_format } format

journal.cli.backfill.marker = Stamped { $path }, but could not mark day { $day } updated: { $error }

journal.cli.backfill.preview = DRY RUN (no changes written)

journal.cli.backfill.segment.directory = Could not list segment directory { $path }: { $error }

journal.cli.backfill.segments = Could not list segments for day { $day }: { $error }

journal.cli.backfill.stamp.changed = Could not stamp { $path }: file changed after classification

journal.cli.backfill.stamp.read = Could not stamp { $path }: could not re-read file: { $error }

journal.cli.backfill.stamp.write = Could not stamp { $path }: write failed: { $error }

journal.cli.backfill.stream.directory = Could not list stream directory { $path }: not a directory

journal.cli.backfill.stream.encoding = segment stream is not { $encoding } representable: { $path }

journal.cli.reprocess.empty.range = no data for days { $day } through { $through }

journal.cli.reprocess.help = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $line_break }Submit a past journal day for reprocessing{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $day_column }Past day in { $date_format } format{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_9 }show this help message and exit{ $line_break }{ $indent_2 }{ $through_option }{ $gap_2 }Inclusive range end in { $date_format } format{ $line_break }{ $syntax_continuation_13 }{ $line_break }{ $indent_2 }{ $unit_option }{ $gap_8 }Request a retry of one failed daily processing step{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_6 }Facet of that unit; required when the unit has a facet{ $line_break }{ $indent_2 }{ $from_scratch_option }{ $gap_5 }Force a full daily re-run, preserving markers (does not{ $line_break }                     flag the day as updated){ $line_break }{ $indent_2 }{ $mark_updated_option }{ $gap_5 }Flag the day as having new raw data so daily processing{ $line_break }                     re-queues it, then nudge a drain{ $line_break }{ $indent_2 }{ $owed_option }{ $gap_13 }List the daily outputs owed on the day or range and why,{ $line_break }                     without submitting anything{ $line_break }{ $indent_2 }{ $json_option }{ $gap_13 }With { $owed_flag }, report each day as states and counts: its{ $line_break }                     markers, its coverage, how many daily outputs it has and{ $line_break }                     how many are owed, by cause{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_6 }Enable verbose output{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_8 }Enable debug logging

journal.cli.reprocess.native.arguments = native { $binary } { $verb } failed: arguments are not valid { $encoding }

journal.cli.reprocess.no.data = no data for day { $day }

journal.cli.reprocess.no.engine = no model is chosen yet. choose one in thinking, then retry

journal.cli.reprocess.owed.only = { $owed_flag } only reports; nothing was submitted

journal.cli.reprocess.owed.summary = { $owed_quantity ->
    [one] { $days_quantity ->
        [one] { $unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
        }
       *[other] { $unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
        }
    }
   *[other] { $days_quantity ->
        [one] { $unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
        }
       *[other] { $unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
        }
    }
}

journal.cli.reprocess.owed.summary.causes = { $owed_quantity ->
    [one] { $days_quantity ->
        [one] { $unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
        }
       *[other] { $unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
        }
    }
   *[other] { $days_quantity ->
        [one] { $unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
        }
       *[other] { $unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
        }
    }
}

journal.cli.reprocess.past.only = reprocess is past-only (cannot reprocess today or a future day)

journal.cli.reprocess.range.plan = { $days_quantity ->
    [one] { $segments_quantity ->
        [one] from-scratch reprocess plan:{ $break }{ $days } day with data ({ $segments } segment) will be queued. Progress will be visible in { $top } or { $health }. Requests survive a supervisor restart; a day that cannot finish from scratch finishes as an ordinary reprocess.{ $break }This day can take hours; today's work goes first between tasks.{ $break }re-run with { $yes } to proceed
       *[other] from-scratch reprocess plan:{ $break }{ $days } day with data ({ $segments } segments) will be queued. Progress will be visible in { $top } or { $health }. Requests survive a supervisor restart; a day that cannot finish from scratch finishes as an ordinary reprocess.{ $break }This day can take hours; today's work goes first between tasks.{ $break }re-run with { $yes } to proceed
    }
   *[other] { $segments_quantity ->
        [one] from-scratch reprocess plan:{ $break }{ $days } days with data ({ $segments } segment) will be queued. Progress will be visible in { $top } or { $health }. Requests survive a supervisor restart; a day that cannot finish from scratch finishes as an ordinary reprocess.{ $break }These days can take hours; today's work goes first between tasks.{ $break }re-run with { $yes } to proceed
       *[other] from-scratch reprocess plan:{ $break }{ $days } days with data ({ $segments } segments) will be queued. Progress will be visible in { $top } or { $health }. Requests survive a supervisor restart; a day that cannot finish from scratch finishes as an ordinary reprocess.{ $break }These days can take hours; today's work goes first between tasks.{ $break }re-run with { $yes } to proceed
    }
}

journal.cli.reprocess.range.saved = { $days_quantity ->
    [one] { $segments_quantity ->
        [one] saved from-scratch requests for { $days } day ({ $segments } segment){ $break }progress is visible in health; today's work goes first
       *[other] saved from-scratch requests for { $days } day ({ $segments } segments){ $break }progress is visible in health; today's work goes first
    }
   *[other] { $segments_quantity ->
        [one] saved from-scratch requests for { $days } days ({ $segments } segment){ $break }progress is visible in health; today's work goes first
       *[other] saved from-scratch requests for { $days } days ({ $segments } segments){ $break }progress is visible in health; today's work goes first
    }
}

journal.cli.reprocess.submitted.from.scratch = reprocess (from-scratch) submitted for { $day }

journal.cli.reprocess.submitted.mark.updated = reprocess (mark-updated) submitted for { $day }

journal.cli.reprocess.submitted.process.now = reprocess (process-now) submitted for { $day }

journal.cli.reprocess.through.order = { $through_flag } must be on or after the start day

journal.cli.reprocess.through.required = { $through_flag } requires { $from_scratch_flag }

journal.cli.reprocess.unit.failed = reprocess unit failed: { $error }

journal.cli.reprocess.unit.saved = retry request saved for { $unit } on { $day }

journal.cli.reprocess.unreadable = { $day }  (unreadable: { $error })

journal.cli.reprocess.usage.conflict = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: error: argument { $option }: not allowed with argument { $previous }

journal.cli.reprocess.usage.missing.day = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: error: the following arguments are required: { $positional }

journal.cli.reprocess.usage.missing.value = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: error: argument { $option }: expected one argument

journal.cli.reprocess.usage.requires = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: error: argument { $option }: requires { $required }

journal.cli.reprocess.usage.unknown = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: error: unrecognized arguments: { $arguments }

journal.cli.reprocess.waiting = request saved; start the background service with { $start_command } to process it

journal.cli.reprocess.intent.busy = requested day is owned by another executor

journal.cli.reprocess.intent.invalid.day = invalid work day

journal.cli.reprocess.intent.uncertain = work intent publication is uncertain: { $outcome }

journal.cli.reprocess.intent.unrecognized = unrecognized work intent

journal.cli.reprocess.owed.internal = { $flag } only reports; use { $function }

journal.cli.reprocess.retry.accepted = this daily output is already accepted

journal.cli.reprocess.retry.disabled = daily talent is no longer enabled

journal.cli.reprocess.retry.invalid.day = invalid retry day

journal.cli.reprocess.retry.invalid.unit = invalid retry unit

journal.cli.reprocess.retry.missing = daily unit is missing

journal.cli.reprocess.retry.missing.evidence = missing held evidence

journal.cli.reprocess.retry.no.longer.held = unit is no longer held

journal.cli.reprocess.retry.not.daily = not daily work

journal.cli.reprocess.retry.state = this daily unit cannot be retried without resolving its current state

journal.cli.reprocess.facet.classification = { $path }: invalid facet classification

journal.cli.reprocess.facet.utf8 = non-{ $encoding } facet

journal.cli.reprocess.input.day = invalid day: { $day }

journal.cli.reprocess.path.utf8 = non-{ $encoding } source path

journal.cli.reprocess.schedule.lookback = daily schedule lookback outside { $range }

journal.cli.reprocess.schema.missing = schema prompt path is missing

journal.cli.reprocess.schema.name = schema must be a filename

journal.cli.reprocess.shape.invalid = invalid shape for { $path }

journal.cli.reprocess.shape.object = shape sidecar is not an object

journal.cli.reprocess.shape.written = invalid written source shape: { $path }

journal.cli.reprocess.source.json.object = { $path } is not a { $encoding } object

journal.cli.reprocess.source.line.error = { $path } line { $line }: { $error }

journal.cli.reprocess.source.line.object = { $path } line { $line } is not an object

journal.cli.reprocess.source.symlink = daily source is a symbolic link: { $path }

journal.cli.reprocess.source.utf8 = non-{ $encoding } daily source path

journal.cli.reprocess.talent.overrides = failed to read talent overrides from { $path }

journal.cli.reprocess.template.utf8 = non-{ $encoding } template name

journal.cli.reprocess.record.invalid = Malformed daily unit record: invalid daily unit identity

journal.cli.reprocess.record.io = I/O error: { $cause }

journal.cli.reprocess.record.mismatch = Malformed daily unit record: identity mismatch in record at { $path }: expected { $expected }, found { $found }

journal.cli.retention.proposal.eligible = policy eligibility: Eligible { $open_brace } anchor: { $anchor }, age_days: { $age_days }, period: Days({ $period_days }) { $close_brace }

journal.cli.retention.proposal.forever = policy eligibility: KeptForever

journal.cli.retention.proposal.no.anchor = policy eligibility: AnchorMissing { $open_brace } anchor: { $anchor } { $close_brace }

journal.cli.retention.proposal.unrecorded = removal failed before a proposal was recorded

journal.cli.retention.proposal.young = policy eligibility: TooYoung { $open_brace } anchor: { $anchor }, age_days: { $age_days }, period: Days({ $period_days }) { $close_brace }

journal.cli.retention.removal.uncertain = deletion couldn't be completed or confirmed. recovery is needed.

journal.cli.retention.recovery.activity.binding = your journal couldn't be checked, so this delete stopped

journal.cli.retention.recovery.activity.busy = your journal is busy with another job

journal.cli.retention.recovery.activity.hold = another journal operation needs attention before this removal can continue

journal.cli.retention.recovery.activity.reserved = this location is reserved for a journal operation and cannot be removed

journal.cli.retention.recovery.path.directory = there is a folder where a recording was expected

journal.cli.retention.recovery.path.invalid = the location does not name a file inside your journal

journal.cli.retention.recovery.path.outside = this is not inside your journal, so it was left alone

journal.cli.retention.recovery.path.permission = the file could not be read or removed, which usually means a permission problem

journal.cli.retention.recovery.path.unavailable = the file could not be removed; the journal was left unchanged

journal.cli.retention.recovery.record.prepare = the record of this removal could not be prepared

journal.cli.retention.recovery.record.write = the record of this removal could not be written

journal.cli.retention.recovery.segment.busy = another process is working on this segment

journal.cli.retention.recovery.segment.gone = this segment has already been removed

journal.cli.retention.recovery.segment.live.occupied = something new was written where this segment was, so it has been left set aside rather than overwritten

journal.cli.retention.recovery.segment.missing = there is no such segment in your journal

journal.cli.retention.recovery.segment.staged = this segment is part-way through removal and is set aside; it is not listed with your other segments until it finishes

journal.cli.retention.recovery.segment.unconfirmed = this could not be confirmed gone, so it is not being reported as removed

journal.cli.retention.recovery.segment.unfinished = a previous removal of this segment did not finish; it needs looking at before this can be retried

journal.cli.segment.argument.missing = usage: { $syntax }{ $line_break }{ $command }: error: unrecognized arguments: the following arguments are required: { $input }

journal.cli.segment.argument.unknown = usage: { $syntax }{ $line_break }{ $command }: error: unrecognized arguments: { $arguments }

journal.cli.segment.day.empty = No segments found for { $day }

journal.cli.segment.help.inspect = usage: { $syntax }{ $break }{ $break }positional arguments:{ $break }{ $option_3 }Segment path: { $example_path } (e.g.{ $break }{ $syntax_line_4 }{ $break }{ $break }options:{ $break }{ $option_7 }show this help message and exit{ $break }{ $option_8 }Output as { $json_format }

journal.cli.segment.help.list = usage: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $syntax_line_2 }{ $break }{ $break }positional arguments:{ $break }{ $option_5 }Day in { $day_format } format{ $break }{ $break }options:{ $break }{ $option_8 }show this help message and exit{ $break }{ $option_9 }Filter to a specific stream{ $break }{ $option_10 }Output as { $json_format }

journal.cli.segment.help.move = usage: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $syntax_line_2 }{ $break }{ $syntax_line_3 }{ $break }{ $break }positional arguments:{ $break }{ $option_6 }Segment path: { $example_path } (e.g.{ $break }{ $syntax_line_7 }{ $break }{ $break }options:{ $break }{ $option_10 }show this help message and exit{ $break }{ $option_11 }Destination day ({ $day_format }){ $break }{ $option_12 }New time ({ $time_format }), preserving original duration{ $break }{ $option_13 }Show plan without making changes{ $break }{ $option_14 }Show detailed progress

journal.cli.segment.help.top = usage: { $syntax }{ $break }{ $break }Inspect and manage journal segments{ $break }{ $break }positional arguments:{ $break }{ $syntax_line_5 }{ $break }{ $option_6 }List segments for a day{ $break }{ $option_7 }Show segment metadata{ $break }{ $option_8 }Verify segment integrity{ $break }{ $option_9 }Move segment to a different day/time{ $break }{ $break }options:{ $break }{ $option_12 }show this help message and exit{ $break }{ $option_13 }Enable verbose output{ $break }{ $option_14 }Enable debug logging

journal.cli.segment.help.verify = usage: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $break }positional arguments:{ $break }{ $option_4 }Segment path: { $example_path }{ $break }{ $break }options:{ $break }{ $option_7 }show this help message and exit{ $break }{ $option_8 }Verify all segments for a day{ $break }{ $option_9 }Output as { $json_format }

journal.cli.segment.missing = Segment not found: { $path }

journal.cli.segment.move.dry = [dry run] No changes made

journal.cli.segment.verify.day.result = { $quantity ->
    [one] Summary: { $passed }/{ $total } check passed
   *[other] Summary: { $passed }/{ $total } checks passed
}

journal.cli.segment.verify.input = { $verb } requires a segment path or { $day_flag }

journal.cli.segment.verify.result = { $quantity ->
    [one] { $passed }/{ $total } check passed
   *[other] { $passed }/{ $total } checks passed
}

journal.cli.segment.check.backward = backward chain

journal.cli.segment.check.chain.no.day = { $prev_segment_key } set without { $prev_day_key }

journal.cli.segment.check.chain.no.successor = next segment not found, not stream tail

journal.cli.segment.check.chain.previous = previous segment found

journal.cli.segment.check.chain.start = no previous segment

journal.cli.segment.check.chain.tail = stream tail

journal.cli.segment.check.content.empty = no { $audio_file }, { $screen_file }, or { $browser_files }

journal.cli.segment.check.content.present = content files present

journal.cli.segment.check.directory.absent = segment directory missing

journal.cli.segment.check.directory.exists = directory exists

journal.cli.segment.check.directory.missing = directory missing

journal.cli.segment.check.forward = forward chain

journal.cli.segment.check.index = index presence

journal.cli.segment.check.index.absent = journal index not available

journal.cli.segment.check.index.error = journal index error: { $error } (run: { $index_command })

journal.cli.segment.check.index.missing = segment not indexed

journal.cli.segment.check.index.present = segment indexed

journal.cli.segment.check.marker.exists = { $stream_file } exists

journal.cli.segment.check.marker.invalid = { $stream_file } invalid { $json_format }

journal.cli.segment.check.marker.missing = { $stream_file } missing

journal.cli.segment.check.marker.no.stream = { $stream_file } missing stream field

journal.cli.segment.check.marker.valid = { $stream_file } valid

journal.cli.segment.check.next = next segment { $path }

journal.cli.segment.check.previous.missing = missing previous segment { $path }

journal.cli.segment.identity.default = segment list refused: named stream directory "{ $default_stream }" cannot be spelled as a record identity: { $path }

journal.cli.segment.identity.duplicate = segment list refused: multiple segments share stream { $stream } key { $key }

journal.cli.segment.identity.encoding = segment list refused: segment path is not { $encoding } representable: { $path }

journal.cli.segment.identity.refused = segment list refused: { $detail }

journal.cli.segment.inspect.chain = Chain:

journal.cli.segment.inspect.events = { $quantity ->
    [one] Events: { $entries } entry
   *[other] Events: { $entries } entries
}

journal.cli.segment.inspect.events.tracts = { $quantity ->
    [one] Events: { $entries } entry ({ $tracts })
   *[other] Events: { $entries } entries ({ $tracts })
}

journal.cli.segment.inspect.files = Files ({ $count }):

journal.cli.segment.inspect.index.error = Index: error ({ $error }) - run: { $index_command }

journal.cli.segment.inspect.index.missing = Index: not-indexed

journal.cli.segment.inspect.index.unavailable = Index: unavailable

journal.cli.segment.inspect.indexed = { $quantity ->
    [one] Index: indexed ({ $chunks } chunk)
   *[other] Index: indexed ({ $chunks } chunks)
}

journal.cli.segment.inspect.next = next: { $next }

journal.cli.segment.inspect.path = Segment: { $path }

journal.cli.segment.inspect.prev = prev: { $previous }

journal.cli.segment.inspect.size = Size: { $size }

journal.cli.segment.inspect.stream = Stream:  { $stream }

journal.cli.segment.inspect.stream.seq = Stream:  { $stream } (seq { $sequence })

journal.cli.segment.inspect.talents = Talents ({ $count }):

journal.cli.segment.inspect.time = Time:    { $range } ({ $seconds }s)

journal.cli.segment.list.heading.dur = DUR

journal.cli.segment.list.heading.files = FILES

journal.cli.segment.list.heading.segment = SEGMENT

journal.cli.segment.list.heading.size = SIZE

journal.cli.segment.list.heading.stream = STREAM

journal.cli.segment.list.heading.talents = TALENTS

journal.cli.segment.list.heading.time = TIME

journal.cli.segment.path.syntax = Segment path must be { $path_form } (e.g. { $example })

journal.cli.segment.status.pass = PASS

journal.cli.segment.move.day.format = Invalid { $to_day_flag } format: { $to_day } (expected { $date_format })

journal.cli.segment.move.directory.created = created directory: { $path }

journal.cli.segment.move.directory.exists = Destination { $destination } already exists; no changes made

journal.cli.segment.move.directory.failed = step 1 directory move failed: { $error }; source and index remain authoritative

journal.cli.segment.move.directory.moved = moved directory: { $source } -> { $destination }

journal.cli.segment.move.events.none = no { $event_file } to rewrite

journal.cli.segment.move.events.rewritten = { $quantity ->
    [one] rewrote { $count } { $event_file } line (day: { $source_day }->{ $destination_day }, segment: { $source }->{ $destination })
   *[other] rewrote { $count } { $event_file } lines (day: { $source_day }->{ $destination_day }, segment: { $source }->{ $destination })
}

journal.cli.segment.move.exists = Segment { $segment } already exists on { $day }/{ $stream }. Use { $to_time_flag } to specify an alternate time.

journal.cli.segment.move.fail.ambiguous = { $quantity ->
    [one] step 3 successor patch inconsistent after move: ambiguous chain: { $count } successor left unpatched
   *[other] step 3 successor patch inconsistent after move: ambiguous chain: { $count } successors left unpatched
}

journal.cli.segment.move.fail.events = step 2 events rewrite inconsistent after move: { $detail }

journal.cli.segment.move.fail.health.marker = step 6 health-marker touch inconsistent after move: { $detail }

journal.cli.segment.move.fail.search = step 5 search update inconsistent after move: the search update could not be queued: { $error }

journal.cli.segment.move.fail.successor = step 3 successor patch inconsistent after move: { $detail }

journal.cli.segment.move.fail.tail.locked = step 4 stream-tail repair inconsistent after move: could not lock stream record

journal.cli.segment.move.fail.tail.malformed = step 4 stream-tail repair inconsistent after move: malformed stream record

journal.cli.segment.move.fail.tail.no.record = step 4 stream-tail repair inconsistent after move: no stream record; identity was not re-authored

journal.cli.segment.move.fail.tail.write = step 4 stream-tail repair inconsistent after move: atomic publication failed

journal.cli.segment.move.markers.touched = touched health markers: { $days }

journal.cli.segment.move.no.duration = Source segment key has no duration

journal.cli.segment.move.no.marker = No { $stream_file } in source segment

journal.cli.segment.move.no.slot = No available segment slot near { $destination_segment } on { $to_day }

journal.cli.segment.move.plan = Move: { $source } -> { $destination }

journal.cli.segment.move.plan.ambiguous = { $quantity ->
    [one] successor to patch: (ambiguous: { $count } successor)
   *[other] successor to patch: (ambiguous: { $count } successors)
}

journal.cli.segment.move.plan.events = { $event_file } lines: { $count }

journal.cli.segment.move.plan.index = index chunks: { $chunks }

journal.cli.segment.move.plan.index.error = index read error: { $error } (delete+reindex will be attempted; run: { $index_command })

journal.cli.segment.move.plan.markers = health markers: { $source_day }, { $destination_day }

journal.cli.segment.move.plan.successor = successor to patch: { $path }

journal.cli.segment.move.plan.tail = successor to patch: (none - stream tail)

journal.cli.segment.move.running = Executing move...

journal.cli.segment.move.same = Source and destination are the same

journal.cli.segment.move.search.queued = search updates queued

journal.cli.segment.move.stream.mismatch = Stream mismatch: path says '{ $path_stream }' but { $stream_file } says '{ $marker_stream }'

journal.cli.segment.move.stream.rebuilt = rebuilt stream state: { $stream }

journal.cli.segment.move.successor.none = no successor to patch (stream tail)

journal.cli.segment.move.successor.patched = patched successor { $path }

journal.cli.segment.move.talents = { $command } will re-run daily talents on the touched days

journal.cli.segment.move.time.format = Invalid { $to_time_flag } format: { $time } (expected { $time_format })

journal.cli.segment.guard.activity = another journal operation needs attention before this change can continue

journal.cli.segment.guard.binding = the journal could not be checked, so this change is on hold

journal.cli.segment.guard.reserved = this location is reserved for a journal operation and cannot be changed

journal.cli.segment.relocation.deleted = segment was deleted by its owner

journal.cli.segment.relocation.location = the segment location does not match the planned move

journal.cli.segment.relocation.marker = could not read marker

journal.cli.segment.inspect.next.none = next: (none)

journal.cli.segment.inspect.next.tail = next: (tail)

journal.cli.segment.inspect.prev.missing = prev: { $path } [MISSING]

journal.cli.segment.inspect.prev.none = prev: (none)

journal.cli.backup.destination.backend.prefix = The repository must start with { $backend }: (for example { $example }).

journal.cli.backup.destination.hosted = { $company } runs your backup storage, so there's no destination to change here. manage it in the services portal.

journal.cli.backup.destination.refused.auth.failed = the destination rejected the credentials. nothing was saved.

journal.cli.backup.destination.refused.bucket.not.found = these credentials can't open a bucket by that name. check the bucket name in the repository. nothing was saved.

journal.cli.backup.destination.refused.locked = the backup there is busy. try again shortly. nothing was saved.

journal.cli.backup.destination.refused.other = the destination couldn't be opened. check the repository and credentials, then try again. nothing was saved.

journal.cli.backup.destination.refused.repo.missing = no backup was found there with these credentials. nothing was saved.

journal.cli.backup.destination.refused.repository.key.refused = there's already a backup there that your recovery key can't open. choose another destination. nothing was saved.

journal.cli.backup.destination.refused.timeout = the destination didn't answer in time. check the repository and credentials, then try again. nothing was saved.

journal.cli.backup.destination.setup.failed = a backup couldn't be set up there ({ $reason }). nothing was saved.

journal.cli.backup.destination.switch.confirm = to switch, run again with { $confirm_new_repository_flag }.

journal.cli.backup.destination.switch.lead = that's a different repository. your existing backup stays where it is, untouched, and your journal won't back up to it or restore from it.

journal.cli.backup.enable.complete = Backup enabled.

journal.cli.backup.enable.complete.key = Backup enabled. Your recovery key:{ $line_break }{ $key_grid }

journal.cli.backup.enable.confirm.key = Your recovery key (write it down - it is the only way to restore):{ $line_break }{ $key_grid }{ $line_break }Confirm by piping the key back: { $command_enable }

journal.cli.backup.enable.destination.required = Set a destination first: { $command_destination_set }

journal.cli.backup.enable.key.mismatch = Recovery key did not match.

journal.cli.backup.enable.repository.missing = Repository not found; pre-initialize it or set { $recovery_setting } in config.

journal.cli.backup.error = Error: { $message }

journal.cli.backup.help.destination = usage: { $syntax }

journal.cli.backup.help.offload = usage: { $syntax }

journal.cli.backup.help.recovery.key = usage: { $syntax }

journal.cli.backup.help.root = usage: { $syntax }

journal.cli.backup.hosted.field = Missing { $name }.

journal.cli.backup.init.code = Repository initialization failed ({ $code_field }{ $returncode }).

journal.cli.backup.init.failed = repository initialization failed.

journal.cli.backup.init.retained = repository initialization failed; retained operation state requires review.

journal.cli.backup.input.io = { $stdin_stream } I/O error.

journal.cli.backup.input.json = invalid { $json_format } on { $stdin_stream }: { $error }

journal.cli.backup.input.limit = { $stdin_stream } request exceeds the { $json_format } input limit.

journal.cli.backup.input.object = expected { $json_format } object on { $stdin_stream }.

journal.cli.backup.offload.preview.held = a read-only offload preview is unavailable. no offload work was started.

journal.cli.backup.offload.restore.day.invalid = Invalid day.

journal.cli.backup.offload.restore.day.or.all = Use either a day or { $all_flag }, not both.

journal.cli.backup.offload.restore.day.required = Provide a day or { $all_flag }.

journal.cli.backup.offload.restore.heading = backup offload restore:

journal.cli.backup.offload.restore.reason.archive.unbound = the backup record doesn't identify the archive needed to restore these files

journal.cli.backup.offload.restore.reason.backup.not.ready = your backup isn't set up, so the offloaded files can't be fetched back

journal.cli.backup.offload.restore.reason.destination.admission.failed = the backup destination couldn't be reached

journal.cli.backup.offload.restore.reason.failed = the restore attempt failed for an unspecified reason

journal.cli.backup.offload.restore.reason.insufficient.free.space = there isn't enough free space on this computer to bring the files back

journal.cli.backup.offload.restore.reason.ledger.degraded = the record of what was offloaded is damaged and can't be used to restore

journal.cli.backup.offload.restore.reason.legacy.read.failed = the backup copy of media offloaded by an earlier version of your journal couldn't be read, so it wasn't restored

journal.cli.backup.offload.restore.reason.legacy.review.held = media offloaded by an earlier version of your journal couldn't be checked against your backup, so it wasn't restored

journal.cli.backup.offload.restore.reason.legacy.verification.failed = the backup copy of media offloaded by an earlier version of your journal didn't match its record, so it wasn't restored

journal.cli.backup.offload.restore.reason.locked = the backup at the destination is locked, usually by a run that's still going or one that was interrupted

journal.cli.backup.offload.restore.reason.missing.file.after.restore = a file that should have come back with the restore is missing

journal.cli.backup.offload.restore.reason.nothing.to.restore = there was nothing offloaded to restore

journal.cli.backup.offload.restore.reason.rclone.unavailable = the { $rclone_program } program isn't available

journal.cli.backup.offload.restore.reason.repo.missing = there's no backup at the destination yet

journal.cli.backup.offload.restore.reason.restic.unavailable = the { $restic_program } backup program isn't available

journal.cli.backup.offload.restore.reason.restore.conflict = a different file is already where at least one of these files belongs, so the file already there was kept and the backup copy wasn't restored

journal.cli.backup.offload.restore.reason.restore.held = restoring offloaded files is temporarily unavailable

journal.cli.backup.offload.restore.reason.restore.not.published = at least one file couldn't be put back in your journal this time and nothing was put in its place, so try again later

journal.cli.backup.offload.restore.reason.segment.identity = one of your journal's recordings couldn't be verified against its own identity record

journal.cli.backup.offload.restore.reason.segment.missing = the journal location these files belong to no longer exists on this computer

journal.cli.backup.offload.restore.reason.stream.marker.failed = the files were restored, but updating the journal's records afterward failed

journal.cli.backup.offload.restore.reason.timeout = it ran too long and stopped

journal.cli.backup.offload.restore.reason.unknown.code = stopped without a reason ({ $code })

journal.cli.backup.offload.restore.reason.verification.failed = a restored file didn't match what it should have been, so it may be damaged

journal.cli.backup.offload.restore.unfinished = the restore didn't finish cleanly, so try again later.

journal.cli.backup.offload.status.heading = backup offload:

journal.cli.backup.offload.status.invalid = invalid offload status.

journal.cli.backup.offload.use.installed = run `{ $command_offload_run }` with the installed { $program } command.

journal.cli.backup.output.json = could not render { $json_format } output.

journal.cli.backup.pause.already = backup is already paused.

journal.cli.backup.pause.complete = backup paused. run `{ $command_resume }` to start it again.

journal.cli.backup.pause.off = backup is off, so there's nothing to pause.

journal.cli.backup.payload.backend = Missing backend.

journal.cli.backup.payload.credential = Missing credential: { $key }.

journal.cli.backup.payload.credentials = Missing credentials.

journal.cli.backup.payload.recovery.key = Missing { $key_field }.

journal.cli.backup.payload.repository = Missing repository.

journal.cli.backup.payload.unsupported = Unsupported backend.

journal.cli.backup.prune.complete = Retention prune complete.

journal.cli.backup.prune.detail.failure.rclone.unavailable = Prune failed: the { $rclone_program } program isn't available. { $detail_field }{ $detail }

journal.cli.backup.prune.detail.failure.restic.unavailable = Prune failed: the { $restic_program } backup program isn't available. { $detail_field }{ $detail }

journal.cli.backup.prune.failure.access = Prune failed: the destination refused permission for this write.

journal.cli.backup.prune.failure.auth.failed = Prune failed: the destination rejected the credentials.

journal.cli.backup.prune.failure.binding.invalid = Prune failed: your journal's backup settings are no longer valid.

journal.cli.backup.prune.failure.binding.superseded = Prune failed: newer backup settings replaced these.

journal.cli.backup.prune.failure.broker.error = Prune failed: the backup service { $company } runs couldn't complete the request.

journal.cli.backup.prune.failure.broker.unreachable = Prune failed: the backup service { $company } runs couldn't be reached.

journal.cli.backup.prune.failure.bucket.not.found = Prune failed: your credentials can't open the bucket the repository names.

journal.cli.backup.prune.failure.failed = Prune failed: no reason was given.

journal.cli.backup.prune.failure.hosted.entitlement.inactive = Prune failed: { $company } isn't running backup for you.

journal.cli.backup.prune.failure.journal.path.unresolved = Prune failed: couldn't find your journal.

journal.cli.backup.prune.failure.locked = Prune failed: the backup at the destination is locked, usually by a run that's still going or one that was interrupted.

journal.cli.backup.prune.failure.rclone.unavailable = Prune failed: the { $rclone_program } program isn't available.

journal.cli.backup.prune.failure.repo.exists = Prune failed: there's already a backup there.

journal.cli.backup.prune.failure.repo.missing = Prune failed: there's no backup at the destination yet.

journal.cli.backup.prune.failure.repository.unsupported = Prune failed: the repository in your backup settings doesn't start with { $s3_backend } or { $b2_backend } to match its backend.

journal.cli.backup.prune.failure.restic.unavailable = Prune failed: the { $restic_program } backup program isn't available.

journal.cli.backup.prune.failure.timeout = Prune failed: it ran too long and stopped.

journal.cli.backup.prune.failure.unknown.code = Prune failed: stopped without a reason ({ $code }).

journal.cli.backup.prune.failure.unreachable = Prune failed: the destination couldn't be reached.

journal.cli.backup.prune.skipped = Prune skipped (not enabled or not configured).

journal.cli.backup.recovery.key.missing = No recovery key is set.

journal.cli.backup.restore.already.set.up = restore couldn't start: your journal is already set up, so it can't be restored over. to use a backup, start a new journal and choose restore during setup.

journal.cli.backup.restore.complete = restore complete: { $counters }, { $integrity_field }{ $integrity }, { $resume_field }{ $resumable }.

journal.cli.backup.restore.degraded.integrity.failed = restore finished with a problem: { $counters }; the backup failed its integrity check and may be damaged, so your journal isn't set to the backup ({ $reason_field }{ $code }).

journal.cli.backup.restore.degraded.integrity.unverified = restore finished with a problem: { $counters }; the integrity check couldn't run (the backup was busy or timed out), so your journal isn't set to the backup ({ $reason_field }{ $code }).

journal.cli.backup.restore.degraded.other = restore finished with a problem: { $counters }; restore completed with degraded evidence ({ $reason_field }{ $code }).

journal.cli.backup.restore.degraded.restore.summary.missing = restore finished with a problem: { $counters }; restore summary was missing or malformed ({ $reason_field }{ $code }).

journal.cli.backup.restore.failed.counters = restore failed: { $counters }.

journal.cli.backup.restore.failure.body.rebuild.failed = restore failed: your journal couldn't be rebuilt from the restored files; { $counters }.

journal.cli.backup.restore.failure.destination.admission.failed = restore failed: the backup destination couldn't be reached; { $counters }.

journal.cli.backup.restore.failure.destination.invalid = restore failed: the backup destination isn't valid; { $counters }.

journal.cli.backup.restore.failure.destination.publish.failed = restore failed: the restored journal couldn't be saved to its destination; { $counters }.

journal.cli.backup.restore.failure.full.scan.failed = restore failed: a full scan of the restored journal failed; { $counters }.

journal.cli.backup.restore.failure.insufficient.free.space = restore failed: there isn't enough free space for this restore, so nothing changed; { $counters }.

journal.cli.backup.restore.failure.integrity.check.io.failed = restore failed: the restored files couldn't be checked; { $counters }.

journal.cli.backup.restore.failure.integrity.failed = restore failed: the backup failed its integrity check and may be damaged, so your journal isn't set to the backup; { $counters }.

journal.cli.backup.restore.failure.integrity.unverified = restore failed: the integrity check couldn't run (the backup was busy or timed out), so your journal isn't set to the backup; { $counters }.

journal.cli.backup.restore.failure.invalid.key = restore failed: the recovery key entered doesn't match this backup; { $counters }.

journal.cli.backup.restore.failure.journal.already.set.up = restore failed: your journal is already set up, so it can't be restored over. to use a backup, start a new journal and choose restore during setup; { $counters }.

journal.cli.backup.restore.failure.journal.snapshot.not.found = restore failed: no backup of your journal was found; { $counters }.

journal.cli.backup.restore.failure.recovery.confirmation.publish.failed = restore failed: the recovery confirmation couldn't be saved; { $counters }.

journal.cli.backup.restore.failure.recovery.key.publish.failed = restore failed: the recovery key couldn't be saved; { $counters }.

journal.cli.backup.restore.failure.restic.unavailable = restore failed: the { $restic_program } backup program isn't available; { $counters }.

journal.cli.backup.restore.failure.restore.failed = restore failed: the restore didn't complete; { $counters }.

journal.cli.backup.restore.failure.restore.io.failed = restore failed: files couldn't be written during restore; { $counters }.

journal.cli.backup.restore.failure.restore.record.failed = restore failed: the restore's result couldn't be recorded; { $counters }.

journal.cli.backup.restore.failure.restore.summary.missing = restore failed: the restore finished but its summary is missing; { $counters }.

journal.cli.backup.restore.failure.snapshot.catalog.invalid = restore failed: the list of backups is invalid; { $counters }.

journal.cli.backup.restore.failure.snapshot.list.failed = restore failed: the list of backups couldn't be retrieved; { $counters }.

journal.cli.backup.restore.failure.snapshot.list.io.failed = restore failed: the list of backups couldn't be read; { $counters }.

journal.cli.backup.restore.failure.snapshot.selection.ambiguous = restore failed: more than one backup matched and none could be chosen automatically; { $counters }.

journal.cli.backup.restore.failure.unknown.code = restore failed: stopped without a reason ({ $code }); { $counters }.

journal.cli.backup.restore.recording.failure.body.rebuild.failed = Recording the result also failed: your journal couldn't be rebuilt from the restored files.

journal.cli.backup.restore.recording.failure.destination.admission.failed = Recording the result also failed: the backup destination couldn't be reached.

journal.cli.backup.restore.recording.failure.destination.invalid = Recording the result also failed: the backup destination isn't valid.

journal.cli.backup.restore.recording.failure.destination.publish.failed = Recording the result also failed: the restored journal couldn't be saved to its destination.

journal.cli.backup.restore.recording.failure.full.scan.failed = Recording the result also failed: a full scan of the restored journal failed.

journal.cli.backup.restore.recording.failure.insufficient.free.space = Recording the result also failed: there isn't enough free space for this restore, so nothing changed.

journal.cli.backup.restore.recording.failure.integrity.check.io.failed = Recording the result also failed: the restored files couldn't be checked.

journal.cli.backup.restore.recording.failure.integrity.failed = Recording the result also failed: the backup failed its integrity check and may be damaged, so your journal isn't set to the backup.

journal.cli.backup.restore.recording.failure.integrity.unverified = Recording the result also failed: the integrity check couldn't run (the backup was busy or timed out), so your journal isn't set to the backup.

journal.cli.backup.restore.recording.failure.invalid.key = Recording the result also failed: the recovery key entered doesn't match this backup.

journal.cli.backup.restore.recording.failure.journal.already.set.up = Recording the result also failed: your journal is already set up, so it can't be restored over. to use a backup, start a new journal and choose restore during setup.

journal.cli.backup.restore.recording.failure.journal.snapshot.not.found = Recording the result also failed: no backup of your journal was found.

journal.cli.backup.restore.recording.failure.recovery.confirmation.publish.failed = Recording the result also failed: the recovery confirmation couldn't be saved.

journal.cli.backup.restore.recording.failure.recovery.key.publish.failed = Recording the result also failed: the recovery key couldn't be saved.

journal.cli.backup.restore.recording.failure.restic.unavailable = Recording the result also failed: the { $restic_program } backup program isn't available.

journal.cli.backup.restore.recording.failure.restore.failed = Recording the result also failed: the restore didn't complete.

journal.cli.backup.restore.recording.failure.restore.io.failed = Recording the result also failed: files couldn't be written during restore.

journal.cli.backup.restore.recording.failure.restore.record.failed = Recording the result also failed: the restore's result couldn't be recorded.

journal.cli.backup.restore.recording.failure.restore.summary.missing = Recording the result also failed: the restore finished but its summary is missing.

journal.cli.backup.restore.recording.failure.snapshot.catalog.invalid = Recording the result also failed: the list of backups is invalid.

journal.cli.backup.restore.recording.failure.snapshot.list.failed = Recording the result also failed: the list of backups couldn't be retrieved.

journal.cli.backup.restore.recording.failure.snapshot.list.io.failed = Recording the result also failed: the list of backups couldn't be read.

journal.cli.backup.restore.recording.failure.snapshot.selection.ambiguous = Recording the result also failed: more than one backup matched and none could be chosen automatically.

journal.cli.backup.restore.recording.failure.unknown.code = Recording the result also failed: stopped without a reason ({ $code }).

journal.cli.backup.restore.unrecorded = restore completed, but recording the result failed: { $counters } ({ $reason_field }{ $restore_record }).

journal.cli.backup.resume.complete = backup resumed. the next backup runs on its usual schedule.

journal.cli.backup.resume.not.paused = backup isn't paused.

journal.cli.backup.rotation.failed = Rotation failed: failed.

journal.cli.backup.rotation.failure.access = Rotation failed: the destination refused permission for this write.

journal.cli.backup.rotation.failure.auth.failed = Rotation failed: the destination rejected the credentials.

journal.cli.backup.rotation.failure.binding.invalid = Rotation failed: your journal's backup settings are no longer valid.

journal.cli.backup.rotation.failure.binding.superseded = Rotation failed: newer backup settings replaced these.

journal.cli.backup.rotation.failure.broker.error = Rotation failed: the backup service { $company } runs couldn't complete the request.

journal.cli.backup.rotation.failure.broker.unreachable = Rotation failed: the backup service { $company } runs couldn't be reached.

journal.cli.backup.rotation.failure.bucket.not.found = Rotation failed: your credentials can't open the bucket the repository names.

journal.cli.backup.rotation.failure.failed = Rotation failed: no reason was given.

journal.cli.backup.rotation.failure.hosted.entitlement.inactive = Rotation failed: { $company } isn't running backup for you.

journal.cli.backup.rotation.failure.journal.path.unresolved = Rotation failed: couldn't find your journal.

journal.cli.backup.rotation.failure.locked = Rotation failed: the backup at the destination is locked, usually by a run that's still going or one that was interrupted.

journal.cli.backup.rotation.failure.rclone.unavailable = Rotation failed: the { $rclone_program } program isn't available.

journal.cli.backup.rotation.failure.repo.exists = Rotation failed: there's already a backup there.

journal.cli.backup.rotation.failure.repo.missing = Rotation failed: there's no backup at the destination yet.

journal.cli.backup.rotation.failure.repository.unsupported = Rotation failed: the repository in your backup settings doesn't start with { $s3_backend } or { $b2_backend } to match its backend.

journal.cli.backup.rotation.failure.restic.unavailable = Rotation failed: the { $restic_program } backup program isn't available.

journal.cli.backup.rotation.failure.timeout = Rotation failed: it ran too long and stopped.

journal.cli.backup.rotation.failure.unknown.code = Rotation failed: stopped without a reason ({ $code }).

journal.cli.backup.rotation.failure.unreachable = Rotation failed: the destination couldn't be reached.

journal.cli.backup.rotation.key = New recovery key (write it down):{ $line_break }{ $key_grid }

journal.cli.backup.rotation.skipped = Recovery key rotation skipped (backup not configured).

journal.cli.backup.run.complete = Backup complete (snapshot { $snapshot }).

journal.cli.backup.run.failure.access = Backup failed: the destination refused permission for this write.

journal.cli.backup.run.failure.auth.failed = Backup failed: the destination rejected the credentials.

journal.cli.backup.run.failure.binding.invalid = Backup failed: your journal's backup settings are no longer valid.

journal.cli.backup.run.failure.binding.superseded = Backup failed: newer backup settings replaced these.

journal.cli.backup.run.failure.broker.error = Backup failed: the backup service { $company } runs couldn't complete the request.

journal.cli.backup.run.failure.broker.unreachable = Backup failed: the backup service { $company } runs couldn't be reached.

journal.cli.backup.run.failure.bucket.not.found = Backup failed: your credentials can't open the bucket the repository names.

journal.cli.backup.run.failure.failed = Backup failed: no reason was given.

journal.cli.backup.run.failure.hosted.entitlement.inactive = Backup failed: { $company } isn't running backup for you.

journal.cli.backup.run.failure.journal.path.unresolved = Backup failed: couldn't find your journal.

journal.cli.backup.run.failure.locked = Backup failed: the backup at the destination is locked, usually by a run that's still going or one that was interrupted.

journal.cli.backup.run.failure.rclone.unavailable = Backup failed: the { $rclone_program } program isn't available.

journal.cli.backup.run.failure.repo.exists = Backup failed: there's already a backup there.

journal.cli.backup.run.failure.repo.missing = Backup failed: there's no backup at the destination yet.

journal.cli.backup.run.failure.repository.unsupported = Backup failed: the repository in your backup settings doesn't start with { $s3_backend } or { $b2_backend } to match its backend.

journal.cli.backup.run.failure.restic.unavailable = Backup failed: the { $restic_program } backup program isn't available.

journal.cli.backup.run.failure.timeout = Backup failed: it ran too long and stopped.

journal.cli.backup.run.failure.unknown.code = Backup failed: stopped without a reason ({ $code }).

journal.cli.backup.run.failure.unreachable = Backup failed: the destination couldn't be reached.

journal.cli.backup.run.partial.snapshot.count = { $quantity ->
    [one] Backup partial (snapshot { $snapshot }). 1 file couldn't be read, so it isn't in it. Run `{ $command }` again once that's fixed.
   *[other] Backup partial (snapshot { $snapshot }). { $count } files couldn't be read, so they aren't in it. Run `{ $command }` again once that's fixed.
}

journal.cli.backup.run.partial.snapshot.count.reason = { $quantity ->
    [one] Backup partial (snapshot { $snapshot }). 1 file couldn't be read, so it isn't in it. The first: { $reason }. Run `{ $command }` again once that's fixed.
   *[other] Backup partial (snapshot { $snapshot }). { $count } files couldn't be read, so they aren't in it. The first: { $reason }. Run `{ $command }` again once that's fixed.
}

journal.cli.backup.run.partial.snapshot.unmeasured = Backup partial (snapshot { $snapshot }). some files couldn't be read, so they aren't in it. Run `{ $command_run }` again once that's fixed.

journal.cli.backup.run.partial.unconfirmed.count = { $quantity ->
    [one] Backup partial. 1 file couldn't be read, so it isn't in it, and the snapshot couldn't be confirmed. Run `{ $command }` again once that's fixed.
   *[other] Backup partial. { $count } files couldn't be read, so they aren't in it, and the snapshot couldn't be confirmed. Run `{ $command }` again once that's fixed.
}

journal.cli.backup.run.partial.unconfirmed.count.reason = { $quantity ->
    [one] Backup partial. 1 file couldn't be read, so it isn't in it, and the snapshot couldn't be confirmed. The first: { $reason }. Run `{ $command }` again once that's fixed.
   *[other] Backup partial. { $count } files couldn't be read, so they aren't in it, and the snapshot couldn't be confirmed. The first: { $reason }. Run `{ $command }` again once that's fixed.
}

journal.cli.backup.run.partial.unconfirmed.unmeasured = Backup partial. some files couldn't be read, so they aren't in it, and the snapshot couldn't be confirmed. Run `{ $command_run }` again once that's fixed.

journal.cli.backup.run.skipped = Backup skipped (not enabled or not configured).

journal.cli.backup.teardown.backup.only.held = your backup holds the only copy of some of your media, so no backup changes were made. restore everything from the backup page first, then try again.

journal.cli.backup.teardown.confirm = Refusing to tear down backup without { $yes_flag }. This forgets all snapshots.

journal.cli.backup.teardown.not.started = turning off and deleting your backup couldn't start. no backup changes were made.

journal.cli.backup.teardown.partial = backup teardown did not finish. keep recovery files and review the recorded effects before retrying.

journal.cli.backup.teardown.prelaunch.held = turning off and deleting your backup isn't available here. no backup changes were made.

journal.cli.backup.usage.destination = { $command }: error: unrecognized arguments: { $arguments }

journal.cli.backup.usage.offload = { $command }: error: unrecognized arguments: { $arguments }

journal.cli.backup.usage.recovery.key = { $command }: error: unrecognized arguments: { $arguments }

journal.cli.backup.usage.root = { $command }: error: unrecognized arguments: { $arguments }

journal.cli.backup.config.canonical.recovery.characters = canonical recovery key contains invalid { $Crockford } characters

journal.cli.backup.config.canonical.recovery.length = canonical recovery key must be exactly 64 characters

journal.cli.backup.config.entropy = could not generate backup key.

journal.cli.backup.config.hosted.delete = could not delete hosted binding.

journal.cli.backup.config.hosted.read = could not read hosted binding.

journal.cli.backup.config.hosted.write = could not save hosted binding.

journal.cli.backup.config.invalid.destination.shape = backup destination must be a { $JSON } object

journal.cli.backup.config.invalid.mode = backup mode must be { $byo } or { $operated }

journal.cli.backup.config.invalid.offload.bytes = backup offload byte values must be positive integers or null

journal.cli.backup.config.invalid.offload.enabled = backup offload enabled must be a boolean

journal.cli.backup.config.invalid.offload.shape = backup offload must include { $enabled }, { $budget_bytes }, { $floor_bytes }

journal.cli.backup.config.invalid.offload.status = backup offload status must be { $ok_value }, { $skipped_value }, { $stalled_value }, or { $error_value }

journal.cli.backup.config.invalid.restore.counters = backup restore counters must be non-negative integers

journal.cli.backup.config.invalid.restore.scope = backup restore scope must be { $day_value }, { $all_value }, or { $journal_value }

journal.cli.backup.config.invalid.restore.status = backup restore status must be { $ok_value }, { $no_op_value }, { $refused_value }, { $degraded_value }, or { $error_value }

journal.cli.backup.config.invalid.retention.shape = backup retention must include { $hourly }, { $daily }, { $weekly }, { $monthly }

journal.cli.backup.config.invalid.retention.value = backup retention values must be non-negative integers

journal.cli.backup.config.invalid.verification.status = backup verification status must be { $ok_value }, { $skipped_value }, or { $error_value }

journal.cli.backup.config.lock.timeout = could not acquire lock for { $path } within { $seconds }s

journal.cli.backup.config.missing.credential = missing backup credential: { $key }

journal.cli.backup.config.recovery.parse = recovery key must contain exactly 64 { $Crockford } characters after cleanup

journal.cli.backup.config.stored.keys = backup keys must be strings when present

journal.cli.backup.config.teardown.recovery.corrupt = teardown recovery record corrupt or invalid.

journal.cli.backup.config.teardown.recovery.delete = could not delete teardown recovery record.

journal.cli.backup.config.teardown.recovery.read = could not read teardown recovery record.

journal.cli.backup.config.teardown.recovery.write = could not save teardown recovery record.

journal.cli.backup.config.unsupported.backend = unsupported backup backend

journal.cli.backup.offload.status.overflow = media status total overflow

journal.cli.backup.offload.media.byte.overflow = media byte total overflow

journal.cli.backup.offload.media.file.overflow = media file total overflow

journal.cli.offload.guidance.changed.retry = offload stopped because your journal changed while it was running. nothing was removed from this computer. run it again.

journal.cli.offload.guidance.changed.review = offload stopped because your journal changed while it was running. nothing was removed from this computer. keep offload's recovery files and review the recorded effects before retrying.

journal.cli.offload.guidance.clock = the operation clock could not be confirmed.

journal.cli.offload.guidance.journal = your journal could not be confirmed. keep its recovery files for review.

journal.cli.offload.guidance.platform = offload execution is unavailable on this platform.

journal.cli.offload.guidance.start = offload did not start.

journal.cli.offload.guidance.tool = use the installed { $solstone } command with its packaged backup tool.

journal.cli.offload.guidance.unfinished = offload did not finish. keep its recovery files and review the recorded effects before retrying.

journal.cli.offload.result.archive.failed = the archive step of the offload didn't complete

journal.cli.offload.result.backup.failing = your last backup didn't complete successfully

journal.cli.offload.result.backup.not.ready = your backup isn't set up or hasn't completed a successful run yet

journal.cli.offload.result.budget.already.satisfied = the offload budget was already met, so nothing needed to be marked for release

journal.cli.offload.result.budget.satisfied = enough media has been marked for release to meet the offload budget

journal.cli.offload.result.confirm.failed = the offload couldn't confirm the archived files actually reached the backup

journal.cli.offload.result.confirm.tool.failed = the check that confirms an offload archive couldn't complete

journal.cli.offload.result.markable.media.exhausted = every eligible file was already checked; nothing more can be marked for release right now

journal.cli.offload.result.other = stopped without a reason ({ $reason })

journal.cli.offload.result.unexpected.error = something unexpected interrupted the offload

journal.cli.offload.result.verification.failed = the last backup verification found a problem with the backup's integrity

journal.cli.offload.result.verification.missing = no backup verification has completed yet

journal.cli.offload.result.verification.overdue = backup verification hasn't succeeded recently enough

journal.cli.offload.stopped.archive.cancelled = offload stopped: the archive operation was cancelled.

journal.cli.offload.stopped.archive.clock.changed = offload stopped: the archive clock moved backward.

journal.cli.offload.stopped.archive.configuration.changed = offload stopped: archive configuration changed.

journal.cli.offload.stopped.archive.configuration.unavailable = offload stopped: archive configuration could not be read.

journal.cli.offload.stopped.archive.credentials.expired = offload stopped: archive credentials expired.

journal.cli.offload.stopped.archive.deadline.exceeded = offload stopped: the archive operation deadline was reached.

journal.cli.offload.stopped.archive.invalid.credential.expiry = offload stopped: archive credentials have an invalid expiry.

journal.cli.offload.stopped.archive.invalid.selection = offload stopped: archive sources must be a nonempty selection of absolute paths.

journal.cli.offload.stopped.archive.journal.identity.changed = offload stopped: your journal could not be confirmed.

journal.cli.offload.stopped.archive.not.configured = offload stopped: archive backup is not configured.

journal.cli.offload.stopped.archive.repository.identity.changed = offload stopped: the backup no longer matches.

journal.cli.offload.stopped.archive.repository.identity.invalid = offload stopped: the backup could not be identified.

journal.cli.offload.stopped.archive.snapshot.identity.invalid = offload stopped: the archive did not return a full snapshot identity.

journal.cli.offload.stopped.archive.source.unavailable = offload stopped: the prepared archive files could not be confirmed.

journal.cli.offload.stopped.archive.tool.unavailable = offload stopped: an archive tool is unavailable.

journal.cli.offload.stopped.creation = offload stopped: making proposals did not finish.

journal.cli.offload.stopped.inputs.io = offload stopped: a file in your journal couldn't be opened or read.

journal.cli.offload.stopped.inputs.space = offload stopped: not enough free space on this computer ({ $available } free, { $required } needed).

journal.cli.offload.stopped.native.start = offload stopped: archive start refused (status { $status }).

journal.cli.offload.stopped.plan.incomplete = offload stopped: preparing proposals did not finish.

journal.cli.offload.stopped.planning = offload stopped: offload couldn't prepare its proposals.

journal.cli.offload.stopped.readback = offload stopped: reading the copy back from your backup failed.

journal.cli.offload.stopped.readback.credentials.unavailable = offload stopped: archive read credentials are unavailable.

journal.cli.offload.stopped.readback.incomplete.discovery = offload stopped: the archive walk did not account for every node.

journal.cli.offload.stopped.readback.invalid.selection = offload stopped: the selected archive file is invalid.

journal.cli.offload.stopped.readback.native.failure = offload stopped: the archived file could not be prepared.

journal.cli.offload.stopped.readback.not.configured = offload stopped: reading archived files is not configured.

journal.cli.offload.stopped.readback.snapshot.node.unavailable = offload stopped: the archived file could not be confirmed.

journal.cli.offload.stopped.readback.target.root.changed = offload stopped: the file could not be prepared in your journal.

journal.cli.offload.stopped.readback.tool.changed = offload stopped: the installed archive tool changed; try again.

journal.cli.offload.stopped.readback.tool.failed = offload stopped: the archive read command failed.

journal.cli.offload.stopped.readback.tool.unavailable = offload stopped: the installed archive tool is unavailable.

journal.cli.offload.stopped.readback.unsupported = offload stopped: reading archived files is unavailable on this platform.

journal.cli.offload.stopped.readback.unsupported.backend = offload stopped: this backup destination does not support reading archived files.

journal.cli.offload.stopped.readback.unsupported.tool.version = offload stopped: the installed archive tool version is unsupported.

journal.cli.offload.stopped.record = offload stopped: saving offload's record failed.

journal.cli.offload.stopped.selection = offload stopped: the selected recordings could not be confirmed.

journal.cli.offload.stopped.temporary.copy = offload stopped: making a temporary copy failed.

journal.cli.offload.stopped.tool.reason = offload stopped: { $reason }.

journal.cli.offload.stopped.unfinished.work = offload stopped: your journal still has unfinished work.
