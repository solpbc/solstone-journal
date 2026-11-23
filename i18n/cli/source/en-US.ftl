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

journal.cli.doctor.backlog.inventory.partial.current.clear = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection
}

journal.cli.doctor.backlog.inventory.partial.last = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; last known progress; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; last known progress; set aside: { $holds }
}

journal.cli.doctor.backlog.inventory.partial.last.clear = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; last known progress
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; last known progress
}

journal.cli.doctor.backlog.inventory.whole.current = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; set aside: { $holds }
}

journal.cli.doctor.backlog.inventory.whole.current.clear = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection
}

journal.cli.doctor.backlog.inventory.whole.last = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; last known progress; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; last known progress; set aside: { $holds }
}

journal.cli.doctor.backlog.inventory.whole.last.clear = { $quantity ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; last known progress
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; last known progress
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

journal.cli.segment.argument.missing = usage: { $syntax }{ $line_break }{ $command }: error: the following arguments are required: { $input }

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

journal.cli.backup.usage.missing.destination = { $command }: error: the following arguments are required: { $required }

journal.cli.backup.usage.missing.offload = { $command }: error: the following arguments are required: { $required }

journal.cli.backup.usage.missing.recovery.key = { $command }: error: the following arguments are required: { $required }

journal.cli.backup.usage.missing.root = { $command }: error: the following arguments are required: { $required }

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

journal.cli.brain.arguments.invalid = { $command }: error: invalid arguments

journal.cli.brain.busy = Thinking busy: a check is already running

journal.cli.brain.check.config = thinking check failed: could not read journal config: { $detail }

journal.cli.brain.check.failure = thinking check failed: { $detail }

journal.cli.brain.check.key.missing = the provider needs a key, and your journal couldn't confirm the result was saved.

journal.cli.brain.check.key.rejected = the provider rejected this key, and your journal couldn't confirm the result was saved.




journal.cli.brain.help.refresh = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }show this help message and exit{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Emit { $json_format } instead of plain output{ $line_break }{ $syntax_continuation_7 }{ $line_break }{ $syntax_continuation_8 }{ $line_break }{ $syntax_continuation_9 }

journal.cli.brain.help.renew = usage: { $syntax }{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }show this help message and exit

journal.cli.brain.help.root = usage: { $syntax }{ $line_break }{ $line_break }Active-brain status and bounded refresh CLI.{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $syntax_continuation_5 }{ $line_break }    { $status }Show active-brain status{ $line_break }    { $refresh }Run one bounded active-brain check{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_10 }show this help message and exit

journal.cli.brain.help.status = usage: { $syntax }{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }show this help message and exit{ $line_break }{ $indent_2 }{ $json_option }{ $gap_6 }Emit { $json_format } instead of plain output

journal.cli.brain.ready.byo = Thinking ready: your own model

journal.cli.brain.ready.byo.checked = Thinking ready: your own model, checked { $age } ago

journal.cli.brain.ready.byo.model = Thinking ready: your own model { $model }

journal.cli.brain.ready.byo.model.checked = Thinking ready: your own model { $model }, checked { $age } ago

journal.cli.brain.ready.local = Thinking ready: local thinking

journal.cli.brain.ready.local.checked = Thinking ready: local thinking, checked { $age } ago

journal.cli.brain.ready.local.model = Thinking ready: local thinking { $model }

journal.cli.brain.ready.local.model.checked = Thinking ready: local thinking { $model }, checked { $age } ago

journal.cli.brain.ready.other = Thinking ready: { $lane }

journal.cli.brain.ready.other.checked = Thinking ready: { $lane }, checked { $age } ago

journal.cli.brain.ready.other.model = Thinking ready: { $lane } { $model }

journal.cli.brain.ready.other.model.checked = Thinking ready: { $lane } { $model }, checked { $age } ago

journal.cli.brain.ready.spp = Thinking ready: confidential processing

journal.cli.brain.ready.spp.checked = Thinking ready: confidential processing, checked { $age } ago

journal.cli.brain.ready.spp.model = Thinking ready: confidential processing { $model }

journal.cli.brain.ready.spp.model.checked = Thinking ready: confidential processing { $model }, checked { $age } ago

journal.cli.brain.ready.unknown = Thinking ready: unknown

journal.cli.brain.ready.unknown.checked = Thinking ready: unknown, checked { $age } ago

journal.cli.brain.ready.unknown.model = Thinking ready: unknown { $model }

journal.cli.brain.ready.unknown.model.checked = Thinking ready: unknown { $model }, checked { $age } ago

journal.cli.brain.usage = usage: { $syntax }

journal.cli.brain.check.open.directory = open flat-directory descendant failed for { $path }: { $reason }

journal.cli.brain.check.random = could not generate secure random bytes

journal.cli.brain.settlement.fingerprint.changed = protected fingerprint key changed after validation

journal.cli.brain.settlement.publication.changed = credential validation changed after publication

journal.cli.brain.settlement.root.changed = admitted journal path changed

journal.cli.brain.settlement.selection.changed = credential selection changed during refresh

journal.cli.brain.settlement.unconfirmed = credential validation publication was not confirmed

journal.cli.brain.settlement.validation.changed = credential validation changed after refresh

journal.cli.brain.heading.blocked = Thinking blocked: { $reason }

journal.cli.brain.heading.blocked.component = Thinking blocked: { $reason } ({ $component })

journal.cli.brain.heading.checking = Thinking checking: { $reason }

journal.cli.brain.heading.checking.component = Thinking checking: { $reason } ({ $component })

journal.cli.brain.heading.other = Thinking { $state }: { $reason }

journal.cli.brain.heading.other.component = Thinking { $state }: { $reason } ({ $component })

journal.cli.brain.heading.unhealthy = Thinking unhealthy: { $reason }

journal.cli.brain.heading.unhealthy.component = Thinking unhealthy: { $reason } ({ $component })

journal.cli.brain.heading.unknown = Thinking unknown: { $reason }

journal.cli.brain.heading.unknown.component = Thinking unknown: { $reason } ({ $component })

journal.cli.brain.validation.checking.aggregate.requires.matching.checking.marker = checking aggregate requires matching checking marker

journal.cli.brain.validation.checking.reason.is.not.valid.component.evidence = checking reason is not valid component evidence

journal.cli.brain.validation.component.status.does.not.match.reason.aggregate = component status does not match reason aggregate

journal.cli.brain.validation.detail = { $field }: { $reason }

journal.cli.brain.validation.diagnostic.enum.value.not.allowed = diagnostic enum value not allowed

journal.cli.brain.validation.diagnostic.key.not.allowed = diagnostic key not allowed

journal.cli.brain.validation.diagnostic.value.must.be.an.enum.string = diagnostic value must be an enum string

journal.cli.brain.validation.does.not.match.aggregate.state = does not match { $token_aggregate_state }

journal.cli.brain.validation.expected.sha256.hex.string = expected { $token_sha_256 } hex string

journal.cli.brain.validation.is.required.when.status.is.ok = is required when status is ok

journal.cli.brain.validation.missing.lane.applicable.evidence.without.higher.priority.cause = missing lane-applicable evidence without higher-priority cause

journal.cli.brain.validation.must.be.a.non.negative.integer = must be a non-negative integer

journal.cli.brain.validation.must.be.a.sha256.hex.digest = must be a { $token_sha_256 } hex digest

journal.cli.brain.validation.must.be.a.string = must be a string

journal.cli.brain.validation.must.be.a.string.or.null = must be a string or null

journal.cli.brain.validation.must.be.an.integer = must be an integer

journal.cli.brain.validation.must.be.an.iso8601.timestamp = must be an { $token_iso_8601 } timestamp

journal.cli.brain.validation.must.be.an.object = must be an object

journal.cli.brain.validation.must.be.evidence.recordable = must be evidence-recordable

journal.cli.brain.validation.must.be.non.empty = must be non-empty

journal.cli.brain.validation.must.be.null.when.aggregate.state.is.ready = must be null when { $token_aggregate_state } is ready

journal.cli.brain.validation.must.be.provider.key.invalid = must be { $token_provider_key_invalid }

journal.cli.brain.validation.must.be.set.when.aggregate.state.is.not.ready = must be set when { $token_aggregate_state } is not ready

journal.cli.brain.validation.non.ok.evidence.requires.reason = non-ok evidence requires reason

journal.cli.brain.validation.not.attempted.is.only.valid.for.generate = { $token_not_attempted } is only valid for generate

journal.cli.brain.validation.not.attempted.reason.must.repeat.causal.prerequisite.reason = { $token_not_attempted } reason must repeat causal prerequisite reason

journal.cli.brain.validation.not.attempted.requires.a.non.ok.prerequisite = { $token_not_attempted } requires a non-ok prerequisite

journal.cli.brain.validation.ok.evidence.requires.null.reason = ok evidence requires null reason

journal.cli.brain.validation.prerequisite.renewal.requires.ok.or.declared.failure = prerequisite renewal requires ok or declared failure

journal.cli.brain.validation.reason.not.allowed.for.evidence.component = reason not allowed for evidence component

journal.cli.brain.validation.record.aggregate.reason.does.not.match.evidence = record aggregate/reason does not match evidence

journal.cli.brain.validation.unknown.field = unknown field

journal.cli.brain.validation.unknown.top.level.field = unknown top-level field

journal.cli.brain.validation.unknown.value = unknown value

journal.cli.brain.validation.unsupported.binding.version = unsupported binding version

journal.cli.brain.validation.unsupported.schema.version = unsupported schema version

journal.cli.brain.writer.active.brain.fingerprint.is.unavailable = active brain fingerprint is unavailable

journal.cli.brain.writer.brain.abandon.reason.is.not.recordable.evidence = brain abandon reason is not recordable evidence

journal.cli.brain.writer.brain.fingerprint.changed = brain fingerprint changed

journal.cli.brain.writer.brain.fingerprint.key.is.unavailable = brain fingerprint key is unavailable

journal.cli.brain.writer.brain.lane.changed = brain lane changed

journal.cli.brain.writer.brain.prerequisite.evidence.is.unsafe = brain prerequisite evidence is unsafe

journal.cli.brain.writer.brain.record.changed.during.inspection = Brain record changed during inspection

journal.cli.brain.writer.brain.refresh.checking.marker.is.absent = brain refresh checking marker is absent

journal.cli.brain.writer.brain.refresh.permit.expired = brain refresh permit expired

journal.cli.brain.writer.brain.refresh.record.revision.changed = brain refresh record revision changed

journal.cli.brain.writer.brain.refresh.revision.changed = brain refresh revision changed

journal.cli.brain.writer.brain.refresh.run.id.changed = brain refresh run id changed

journal.cli.brain.writer.brain.runtime.failure.marker.changed = brain runtime failure marker changed

journal.cli.brain.writer.configuration.rejection.changed.after.publication = configuration rejection changed after publication

journal.cli.brain.writer.credential.authority.changed.during.inspection = credential authority changed during inspection

journal.cli.brain.writer.credential.authority.changed.during.observation = credential authority changed during observation

journal.cli.brain.writer.credential.authority.is.not.an.object = credential authority is not an object

journal.cli.brain.writer.credential.authority.is.unreadable = credential authority is unreadable

journal.cli.brain.writer.credential.binding.authority.could.not.be.read.error = credential binding authority could not be read: { $error }

journal.cli.brain.writer.credential.hold.authority.changed = credential hold authority changed

journal.cli.brain.writer.credential.hold.authority.is.not.an.object = credential hold authority is not an object

journal.cli.brain.writer.credential.hold.authority.is.unreadable.error = credential hold authority is unreadable: { $error }

journal.cli.brain.writer.credential.observation.authority.is.not.an.object = credential observation authority is not an object

journal.cli.brain.writer.credential.observation.authority.is.unreadable = credential observation authority is unreadable

journal.cli.brain.writer.credential.observation.changed.during.refresh = credential observation changed during refresh

journal.cli.brain.writer.credential.observation.changed.during.work = credential observation changed during work

journal.cli.brain.writer.credential.observation.identity.is.invalid = credential observation identity is invalid

journal.cli.brain.writer.credential.observation.lacks.original.binding = credential observation lacks original binding

journal.cli.brain.writer.credential.observations.are.not.an.object = credential observations are not an object

journal.cli.brain.writer.credential.process.state.changed.during.inspection = credential process state changed during inspection

journal.cli.brain.writer.credential.publication.reservation.is.absent = credential publication reservation is absent

journal.cli.brain.writer.credential.rejection.changed.after.publication = credential rejection changed after publication

journal.cli.brain.writer.credential.rejection.changed.during.refresh = credential rejection changed during refresh

journal.cli.brain.writer.credential.rejection.changed.during.validation = credential rejection changed during validation

journal.cli.brain.writer.field.must.be.an.object = { $field } must be an object

journal.cli.brain.writer.protected.fingerprint.key.changed.after.rejection = protected fingerprint key changed after rejection

journal.cli.brain.writer.protected.fingerprint.key.changed.during.admission = protected fingerprint key changed during admission

journal.cli.brain.writer.protected.fingerprint.key.changed.during.inspection = protected fingerprint key changed during inspection

journal.cli.brain.writer.protected.fingerprint.key.changed.during.validation = protected fingerprint key changed during validation

journal.cli.brain.writer.protected.fingerprint.key.is.absent.while.credential.authority.exists = protected fingerprint key is absent while credential authority exists

journal.cli.brain.writer.protected.fingerprint.key.is.unavailable = protected fingerprint key is unavailable

journal.cli.brain.writer.reason.is.not.recordable.evidence = reason is not recordable evidence

journal.cli.brain.writer.rejected.refresh.receipt.does.not.match.its.probe = rejected refresh receipt does not match its probe

journal.cli.brain.writer.rejection.receipt.root.provider.changed = rejection receipt root/provider changed

journal.cli.brain.writer.settled.credential.observation.changed.during.work = settled credential observation changed during work

journal.cli.brain.writer.validation.result.does.not.match.its.credential = validation result does not match its credential

journal.cli.brain.writer.validation.result.is.not.an.object = validation result is not an object

journal.cli.brain.writer.protected.fingerprint.key.is.absent.while.a.credential.binding.exists = protected fingerprint key is absent while a credential binding exists

journal.cli.brain.hold.capacity = credential hold capacity is unavailable

journal.cli.brain.hold.invalid.provider = unsupported credential hold provider

journal.cli.brain.hold.root.changed = admitted journal root changed

journal.cli.brain.hold.superseded = credential validation was superseded

journal.cli.brain.hold.unavailable = credential hold state is unavailable

journal.cli.brain.check.fingerprint.options = expected active fingerprint and expected absence are mutually exclusive

journal.cli.talent.error.list.choice = usage: { $syntax }{ $line_break }{ $program }: error: argument { $flag }: invalid choice: '{ $value }' (choose from { $choices })

journal.cli.talent.error.list.unknown = usage: { $syntax }{ $line_break }{ $program }: error: unrecognized arguments: { $arguments }

journal.cli.talent.error.list.value = usage: { $syntax }{ $line_break }{ $program }: error: argument { $flag }: expected one argument

journal.cli.talent.error.log.required = usage: { $syntax }{ $line_break }{ $program }: error: the following arguments are required: { $arguments }

journal.cli.talent.error.logs.count = usage: { $syntax }{ $line_break }{ $program }: error: argument { $count_options }: invalid int value: '{ $value }'

journal.cli.talent.error.logs.value = usage: { $syntax }{ $line_break }{ $program }: error: argument { $flag }: expected one argument

journal.cli.talent.error.passages.required = usage: { $syntax }{ $line_break }{ $program }: error: the following arguments are required: { $arguments }

journal.cli.talent.error.passages.unknown = usage: { $syntax }{ $line_break }{ $program }: error: unrecognized arguments: { $arguments }

journal.cli.talent.error.passages.value = usage: { $syntax }{ $line_break }{ $program }: error: argument { $flag }: expected one argument

journal.cli.talent.error.root.choice = usage: { $syntax }{ $line_break }{ $program }: error: argument { $subcommand }: invalid choice: '{ $value }' (choose from { $choices })

journal.cli.talent.error.root.encoding = usage: { $syntax }{ $line_break }{ $program }: error: arguments are not valid { $encoding }

journal.cli.talent.error.root.unknown = usage: { $syntax }{ $line_break }{ $program }: error: unrecognized arguments: { $arguments }

journal.cli.talent.error.show.required = usage: { $syntax }{ $line_break }{ $program }: error: the following arguments are required: { $arguments }

journal.cli.talent.error.show.unknown = usage: { $syntax }{ $line_break }{ $program }: error: unrecognized arguments: { $arguments }

journal.cli.talent.error.show.value = usage: { $syntax }{ $line_break }{ $program }: error: argument { $flag }: expected one argument

journal.cli.talent.help.list = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }show this help message and exit{ $line_break }{ $syntax_continuation_5 }{ $line_break }                        Filter by schedule type{ $line_break }{ $syntax_continuation_7 }{ $line_break }                        Filter by origin{ $line_break }{ $indent_2 }{ $disabled_option }{ $gap_12 }Include disabled prompts{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Output as { $jsonl }

journal.cli.talent.help.log = usage: { $syntax }{ $line_break }{ $line_break }positional arguments:{ $line_break }  { $id }Agent ID{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }show this help message and exit{ $line_break }{ $indent_2 }{ $json_option }{ $gap_6 }Output raw { $jsonl }{ $line_break }{ $indent_2 }{ $full_option }{ $gap_6 }Expand event details

journal.cli.talent.help.logs = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $line_break }positional arguments:{ $line_break }  { $agent }Filter to a specific agent{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }show this help message and exit{ $line_break }{ $syntax_continuation_9 }{ $line_break }                        Number of runs to show (default: 20){ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }Show only runs from this day{ $line_break }{ $indent_2 }{ $daily_option }{ $gap_15 }Show only daily-scheduled runs{ $line_break }{ $indent_2 }{ $errors_option }{ $gap_14 }Show only error runs{ $line_break }{ $indent_2 }{ $summary_option }{ $gap_13 }Show grouped summary

journal.cli.talent.help.passages = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }                           name{ $line_break }{ $line_break }Copy exact passages from the source an activity talent reads.{ $line_break }A short source unit is kept whole without a model call; a longer one{ $line_break }takes one completion to choose what to copy. No passages are written to{ $line_break }the journal; a completion's usage is logged like any other.{ $line_break }{ $line_break }positional arguments:{ $line_break }  { $name }Activity talent name, such as work or conversation{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }show this help message and exit{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Output the passage document as { $json }{ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }Day of the activity{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_10 }Facet of the activity{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_9 }Activity ID

journal.cli.talent.help.root = usage: { $syntax }{ $line_break }{ $line_break }Inspect talent prompt configurations{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $syntax_continuation_5 }{ $line_break }    { $list }List prompts grouped by schedule{ $line_break }    { $show }Show details for a specific prompt{ $line_break }    { $passages }Copy source passages from an activity's source{ $line_break }    { $logs }Show recent talent run log{ $line_break }    { $log }Show events for an agent run{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }show this help message and exit{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_9 }Enable verbose output{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_11 }Enable debug logging

journal.cli.talent.help.show = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }                           name{ $line_break }{ $line_break }positional arguments:{ $line_break }  { $name }Prompt name{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }show this help message and exit{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Output as { $jsonl }{ $line_break }{ $indent_2 }{ $prompt_option }{ $gap_14 }Show full prompt context (dry-run mode){ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }Day for prompt context{ $line_break }{ $indent_2 }{ $segment_option }{ $gap_2 }Segment for segment-scheduled prompts{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_10 }Facet for multi-facet prompts{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_9 }Activity ID for activity-scheduled prompts{ $line_break }{ $indent_2 }{ $query_option }{ $gap_10 }Sample query for tool agents{ $line_break }{ $indent_2 }{ $full_option }{ $gap_16 }Show full content without truncation

journal.cli.talent.startup.executable = talent failed: could not inspect current executable: { $error }

journal.cli.talent.startup.package = talent failed: could not locate installed { $package } package

journal.cli.talent.config.access.retired = Prompt '{ $talent }' sets retired { $access_tier }

journal.cli.talent.config.activities = Activity-scheduled prompt '{ $talent }' must have a non-empty { $activities } list (activity types to match, or { $all_types } for all types).

journal.cli.talent.config.budget.invalid = Prompt '{ $talent }' has invalid { $max_output_tokens } { $value }. Expected a positive integer measured against the bundled model.

journal.cli.talent.config.budget.required = Prompt '{ $talent }' is missing required { $max_output_tokens }. Measure its largest output on the bundled model and declare 1.5 times that, rounded up to a multiple of 256.

journal.cli.talent.config.budget.retired = Prompt '{ $talent }' declares retired { $thinking_budget }. Thinking is set only in Thinking, for the owner's own model.

journal.cli.talent.config.cwd.retired = Prompt '{ $talent }' sets retired { $cwd }

journal.cli.talent.config.frontmatter = failed to parse frontmatter from { $path }

journal.cli.talent.config.output.required = Prompt '{ $talent }' has type={ $generate } but is missing required { $output } field.

journal.cli.talent.config.priority = Scheduled prompt '{ $talent }' is missing required { $priority } field. All prompts with { $schedule } must declare an explicit priority.

journal.cli.talent.config.read = failed to read { $path }: { $error }

journal.cli.talent.config.stat = failed to stat { $path }: { $error }

journal.cli.talent.config.stem = talent filename has no { $encoding } stem: { $path }

journal.cli.talent.config.type.invalid = Prompt '{ $talent }' has invalid type { $value }. Expected { $generate }.

journal.cli.talent.config.type.required = Prompt '{ $talent }' has output but is missing required { $type } field.

journal.cli.talent.last.age = { $age } ago

journal.cli.talent.last.age.duration = { $age } ago ({ $duration })

journal.cli.talent.list.empty = No prompts found matching filters.

journal.cli.talent.list.group.activity = activity:

journal.cli.talent.list.group.cadence = cadence:

journal.cli.talent.list.group.daily = daily:

journal.cli.talent.list.group.segment = segment:

journal.cli.talent.list.group.unscheduled = unscheduled:

journal.cli.talent.list.group.weekly = weekly:

journal.cli.talent.list.header.last = LAST RUN

journal.cli.talent.list.header.name = NAME

journal.cli.talent.list.header.tags = TAGS

journal.cli.talent.list.header.title = TITLE

journal.cli.talent.list.tag.disabled = disabled

journal.cli.talent.list.tag.fail = FAIL

journal.cli.talent.list.visible = { $count ->
    [one] { $count } prompt ({ $hidden } disabled hidden, use { $disabled_flag })
   *[other] { $count } prompts ({ $hidden } disabled hidden, use { $disabled_flag })
    }

journal.cli.talent.log.event.thinking = think

journal.cli.talent.log.event.tool = tool

journal.cli.talent.log.event.updated = updated

journal.cli.talent.log.missing = Talent run not found: { $run }

journal.cli.talent.log.tokens = { $result } [{ $input }in/{ $output }out]

journal.cli.talent.logs.date = { $month } { $day_time }

journal.cli.talent.logs.day.invalid = Invalid { $day_flag } format: { $day }. Expected { $day_format }.

journal.cli.talent.logs.total = total

journal.cli.talent.passages.incomplete = some source units have no passages; see each unit's outcome

journal.cli.talent.passages.unavailable = source passages unavailable: { $detail }

journal.cli.talent.preview.all.lines = (use { $full_flag } to see all { $count } lines)

journal.cli.talent.preview.banner = Preview of what this talent would send. Same assembly a real run uses; the model is not called.

journal.cli.talent.preview.effective = Effective prompt for: { $talent }

journal.cli.talent.preview.empty = (empty)

journal.cli.talent.preview.instruction = INSTRUCTION

journal.cli.talent.preview.no.day = No day given, so this preview has no day's recordings in it. Pass { $day_flag } { $day_format } to include them.

journal.cli.talent.preview.omitted = { $count ->
    [one] ... ({ $count } line omitted)
   *[other] ... ({ $count } lines omitted)
    }

journal.cli.talent.preview.omitted.legacy = ... ({ $count } lines omitted)

journal.cli.talent.preview.prestep = Prompt preview cannot assemble this talent: a required pre-step is not available.

journal.cli.talent.preview.skipped = This talent would not run: { $reason }

journal.cli.talent.show.body = body:

journal.cli.talent.show.body.lines = { $count ->
    [one] { $count } line
   *[other] { $count } lines
    }

journal.cli.talent.show.hook = Prompt '{ $talent }' is a hook prompt and cannot be run directly.

journal.cli.talent.show.missing = Prompt not found: { $talent }

journal.cli.talent.show.missing.path = Prompt not found: { $talent }{ $line_break }  looked at: { $path }

journal.cli.talent.show.variables = variables:

journal.cli.talent.compose.facet.missing = facet '{ $facet }' not found

journal.cli.talent.compose.prompt.path.has.no.parent = talent { $talent }: prompt path has no parent

journal.cli.talent.compose.prompt.path.is.missing = talent { $talent }: prompt path is missing

journal.cli.talent.compose.schema.directory.is.unavailable = talent { $talent }: schema directory is unavailable: { $error }

journal.cli.talent.compose.schema.file.could.not.be.read = talent { $talent }: schema file could not be read: { $error }

journal.cli.talent.compose.schema.file.is.not.a.valid.json.schema = talent { $talent }: schema file is not a valid { $schema_format }: { $path }

journal.cli.talent.compose.schema.file.is.not.valid.json = talent { $talent }: schema file is not valid { $json }: { $path }

journal.cli.talent.compose.schema.file.not.found.cause = talent { $talent }: schema file not found: { $error }

journal.cli.talent.compose.schema.file.not.found.path = talent { $talent }: schema file not found: { $path }

journal.cli.talent.compose.schema.must.be.a.string.got = talent { $talent }: schema must be a string, got { $type_name }: { $value }

journal.cli.talent.compose.schema.path.escapes.talent.directory = talent { $talent }: schema path escapes talent directory: { $path }

journal.cli.talent.compose.schema.path.must.be.relative = talent { $talent }: schema path must be relative: { $path }

journal.cli.talent.compose.schema.path.must.not.contain = talent { $talent }: schema path must not contain '..': { $path }

journal.cli.talent.compose.settings = failed to read journal config: { $detail }

journal.cli.talent.passages.generate = source passages could not reach generate: { $error }

journal.cli.talent.passages.unit.failed = # unit { $index } · { $kind } · no passages: { $reason }

journal.cli.talent.passages.unit.failed.time = # unit { $index } · { $kind } · { $time } · no passages: { $reason }

journal.cli.talent.passages.unit.not.attempted = # unit { $index } · { $kind } · not attempted

journal.cli.talent.passages.unit.not.attempted.time = # unit { $index } · { $kind } · { $time } · not attempted

journal.cli.talent.passages.unit.passages = { $count ->
    [one] # unit { $index } · { $kind } · { $count } passage
   *[other] # unit { $index } · { $kind } · { $count } passage(s)
    }

journal.cli.talent.passages.unit.passages.time = { $count ->
    [one] # unit { $index } · { $kind } · { $time } · { $count } passage
   *[other] # unit { $index } · { $kind } · { $time } · { $count } passage(s)
    }

journal.cli.talent.passages.verbatim = Verbatim from source:

journal.cli.talent.prepare.configuration.name.declares.an.invalid.mobile.local.timeout = configuration '{ $name }' declares an invalid { $timeout }

journal.cli.talent.prepare.invalid.config.must.have.type.day.or.prompt = Invalid config: must have { $type }, { $day }, or { $prompt }

journal.cli.talent.prepare.invalid.config.segment.or.span.requires.day = Invalid config: { $segment } or { $span } requires { $day }

journal.cli.talent.prepare.invalid.talent.type.kind = invalid talent type '{ $kind }'

journal.cli.talent.prepare.request.field.field.is.retired = request field '{ $field }' is retired

journal.cli.talent.prepare.request.overrides.field.for.talent.name.are.not.allowed = Request overrides '{ $field }' for talent '{ $name }' are not allowed ({ $declared } != { $requested })

journal.cli.talent.prepare.request.overrides.for.field.are.not.allowed = request overrides for '{ $field }' are not allowed; thinking provider and model are configured only in Thinking

journal.cli.talent.prepare.request.overrides.for.field.are.not.allowed.budget = request overrides for '{ $field }' are not allowed; a talent's budgets are its own

journal.cli.talent.prepare.talent.name.declares.field.in.frontmatter = talent '{ $name }' declares '{ $field }' in frontmatter; thinking provider and model are configured only in Thinking

journal.cli.talent.prepare.talent.name.declares.outbound.approval.in.frontmatter = talent '{ $name }' declares { $outbound_approval } in frontmatter; this field is launch-config-only and may not come from a talent definition

journal.cli.talent.prepare.talent.name.not.found = talent '{ $name }' not found

journal.cli.talent.prepare.talent.request.missing.name = talent request missing name

journal.cli.talent.preview.recovery.choose.a.completed.owner.activity.not.an.anticipated.or.cogi = Choose a completed owner activity, not an anticipated or cogitate record.

journal.cli.talent.preview.recovery.choose.a.talent.configured.for.this.activity.kind = Choose a talent configured for this activity kind.

journal.cli.talent.preview.recovery.choose.a.talent.that.loads.its.activity.s.sources = Choose a talent that loads its activity's sources.

journal.cli.talent.preview.recovery.choose.a.talent.that.reads.its.activity.source.without.a.pre = Choose a talent that reads its activity source without a pre-step.

journal.cli.talent.preview.recovery.choose.a.talent.whose.required.pre.step.is.available.in.the = Choose a talent whose required pre-step is available in the native runtime.

journal.cli.talent.preview.recovery.choose.a.talent.whose.schedule.is.activity = Choose a talent whose schedule is { $schedule }

journal.cli.talent.preview.recovery.choose.an.activity.id.stored.in.the.requested.day.and.facet = Choose an activity ID stored in the requested day and facet.

journal.cli.talent.preview.recovery.choose.an.activity.with.at.least.one.recorded.segment = Choose an activity with at least one recorded segment.

journal.cli.talent.preview.recovery.choose.either.activity.or.segment.not.both = Choose either { $activity_flag } or { $segment_flag }, not both.

journal.cli.talent.preview.recovery.correct.the.activity.preview.input.then.retry.detail = Correct the activity preview input, then retry: { $detail }

journal.cli.talent.preview.recovery.correct.the.talent.configuration.detail = Correct the talent configuration: { $detail }

journal.cli.talent.preview.recovery.make.the.selected.activity.and.talent.eligible.then.retry = Make the selected activity and talent eligible, then retry.

journal.cli.talent.preview.recovery.make.the.selected.talent.eligible.then.retry = Make the selected talent eligible, then retry.

journal.cli.talent.preview.recovery.pass.day.yyyymmdd.facet.name.and.activity.id = Pass { $day_flag } { $yyyymmdd }, { $facet_flag } { $name } and { $activity_flag } ID.

journal.cli.talent.preview.recovery.pass.day.yyyymmdd.with.activity = Pass { $day_flag } { $yyyymmdd } with { $activity_flag }.

journal.cli.talent.preview.recovery.pass.facet.name.with.activity = Pass { $facet_flag } { $name } with { $activity_flag }.

journal.cli.talent.preview.recovery.remove.duplicate.records.with.this.id.in.the.requested.day.a = Remove duplicate records with this ID in the requested day and facet.

journal.cli.talent.preview.recovery.repair.the.activity.span.so.every.segment.is.a.non.empty.str = Repair the activity span so every segment is a non-empty string.

journal.cli.talent.preview.recovery.restore.a.readable.activity.record.file.then.retry.detail = Restore a readable activity record file, then retry: { $detail }

journal.cli.talent.preview.recovery.restore.a.readable.segment.directory.then.retry.detail = Restore a readable segment directory, then retry: { $detail }

journal.cli.talent.preview.recovery.restore.readable.segment.directories.then.retry.detail = Restore readable segment directories, then retry: { $detail }

journal.cli.talent.preview.recovery.restore.the.selected.activity.segment.then.retry = Restore the selected activity segment, then retry.

journal.cli.talent.passages.generate.missing = source passages could not reach generate: missing sibling executable { $path }

journal.cli.talent.passages.generate.parent = source passages could not reach generate: current executable has no parent

journal.cli.talent.facet.malformed = malformed data in { $path }

journal.cli.talent.facet.malformed.line = malformed data in { $path } at line { $line }

journal.cli.talent.facet.object = facet declaration is not an object: { $path }

journal.cli.talent.path.absolute = journal path must be relative

journal.cli.talent.path.backslash = journal path must use forward slashes

journal.cli.talent.path.component = journal path contains invalid component

journal.cli.talent.path.day = day must be { $day_format }

journal.cli.talent.path.empty = journal path must not be empty

journal.cli.talent.path.escape = { $relative } escapes the journal root (resolved to { $path })

journal.cli.talent.path.unreadable = journal path is not readable

journal.cli.talent.floor.invalid.budget = missing or invalid { $budget }


journal.cli.talent.prepare.memory.incomplete = private memory context is incomplete

journal.cli.talent.prepare.segment.missing = { $count ->
    [one] Segment directory not found: { $segments }
   *[other] Segment directories not found: { $segments }
    }

journal.cli.talent.work.duration = invalid Work source duration: { $error }

journal.cli.talent.work.duration.range = Work source duration is out of range

journal.cli.talent.work.interval = invalid Work source interval: { $error }

journal.cli.talent.work.interval.range = Work source interval is out of range

journal.cli.talent.work.key = invalid Work source segment: { $segment }

journal.cli.talent.work.read = cannot read Work segment results: { $error }

journal.cli.talent.work.unavailable = Work source segment is unavailable: { $day }/{ $segment }

journal.cli.talent.floor.mismatch = { $entries ->
        [one] entry count mismatch: parsed { $entries } entry but have { $sources ->
                [one] { $sources } entry source
               *[other] { $sources } entry sources
            }
       *[other] entry count mismatch: parsed { $entries } entries but have { $sources ->
                [one] { $sources } entry source
               *[other] { $sources } entry sources
            }
    }

journal.cli.talent.hook.briefing.date.overflow = briefing date overflow

journal.cli.talent.hook.briefing.failed = morning briefing pre-hook failed: { $error }

journal.cli.talent.hook.budget = prepared request does not fit the floor input budget

journal.cli.talent.hook.daily.lookback = daily schedule lookback outside { $range }

journal.cli.talent.hook.day.invalid = invalid or unparseable day: '{ $day }'

journal.cli.talent.hook.day.missing = missing day

journal.cli.talent.hook.document.stream = not a document import segment

journal.cli.talent.hook.entity.minimum = entity packet for { $entity } is at least { $count } characters; maximum is { $maximum }

journal.cli.talent.hook.entity.name = missing entity name

journal.cli.talent.hook.entity.size = entity packet for { $entity } is { $count } characters; maximum is { $maximum }

journal.cli.talent.hook.facet.missing = missing facet

journal.cli.talent.hook.facts.size = entity facts context is { $count } characters; maximum is { $maximum }

journal.cli.talent.hook.failure = { $phase } hook '{ $stage }' for talent '{ $talent }': { $detail }

journal.cli.talent.hook.possible.no.prompt = the talent has no prompt

journal.cli.talent.hook.review.changed = review inputs changed during preparation

journal.cli.talent.hook.suggest.size = entity suggest context is { $count } characters; maximum is { $maximum }

journal.cli.talent.hook.suggestions.array = suggestions JSON missing { $entities } array

journal.cli.talent.hook.suggestions.json = invalid suggestions JSON in { $path }: { $error }

journal.cli.talent.hook.suggestions.missing = missing suggestions artifact: { $path }

journal.cli.talent.hook.sunday = request day { $day } is not a Sunday

journal.cli.talent.hook.transcript.read = transcript { $file } could not be read: { $error }

journal.cli.talent.hook.transcript.utf8 = transcript { $file } is not UTF-8: { $error }

journal.cli.talent.hook.week.incomplete = week ending { $end } has not completed before today { $today }

journal.cli.talent.hook.facet.read = facet { $facet } could not be read: { $error }

journal.cli.talent.hook.facets.list = facets could not be listed: { $error }

journal.cli.talent.hook.identities.read = entity identities could not be read: { $error }

journal.cli.talent.hook.identity.object = entity identity is not an object: { $path }

journal.cli.talent.hook.stream.layout = direct layout requires stream "{ $default_stream }", got "{ $stream }"

journal.cli.talent.hook.transcript.bytes = transcript bytes are not valid UTF-8

journal.cli.talent.hook.lookup.control = invalid path component '{ $candidate }': the name contains a control character

journal.cli.talent.hook.lookup.dotcomponent = invalid path component '{ $candidate }': { $dot } and { $parent } are not valid names

journal.cli.talent.hook.lookup.empty = invalid path component '{ $candidate }': the name is empty

journal.cli.talent.hook.lookup.kind = { $path } is not a directory ({ $kind })

journal.cli.talent.hook.lookup.layout = direct layout requires stream "{ $default_stream }", got { $stream }

journal.cli.talent.hook.lookup.rootorprefix = invalid path component '{ $candidate }': the name must be a single relative component

journal.cli.talent.hook.lookup.separator = invalid path component '{ $candidate }': the name contains a path separator

journal.cli.talent.hook.resolution.absent = resolved ambiguity { $ambiguity } names entity { $entity } absent from this resolution call

journal.cli.talent.hook.resolution.hidden = resolved ambiguity { $ambiguity } names hidden entity { $entity }

journal.cli.think.argument.integer = argument { $option }: invalid int value

journal.cli.think.argument.unknown = unrecognized arguments: { $argument }

journal.cli.think.argument.value = argument { $option }: expected one argument

journal.cli.think.error = { $command }: { $message }

journal.cli.think.help = usage: { $syntax }

journal.cli.think.host.sense.missing = hosted Sense executor is unavailable

journal.cli.think.sense.conflict = { $sense_batch_flag } is incompatible with { $options }

journal.cli.think.sense.day = { $sense_batch_flag } requires { $day_flag }

journal.cli.think.updated.conflict = { $updated_flag } is incompatible with { $options }

journal.cli.think.usage.error = usage: { $syntax }{ $line_break }{ $command }: error: { $message }

journal.cli.think.validation.activity.incompatible = { $activity_flag } is incompatible with { $segment_flag }, { $segments_flag }, and { $flush_flag }

journal.cli.think.validation.activity.requires.day = { $activity_flag } requires { $day_flag }

journal.cli.think.validation.activity.requires.facet = { $activity_flag } requires { $facet_flag }

journal.cli.think.validation.cadence.incompatible = { $cadence_flag } is incompatible with { $segment_flag }, { $segments_flag }, { $activity_flag }, { $flush_flag }, and { $weekly_flag }

journal.cli.think.validation.daily.only = { $daily_only_flag } requires { $day_flag } and cannot be combined with another thinking mode

journal.cli.think.validation.facet.requires.activity = { $facet_flag } requires { $activity_flag }

journal.cli.think.validation.flush.incompatible = { $flush_flag } is incompatible with { $segments_flag } and { $refresh_flag }

journal.cli.think.validation.flush.requires.segment = { $flush_flag } requires { $segment_flag }

journal.cli.think.validation.multi.worker.unlimited.jobs = { $jobs_flag } 0 is incompatible with multi-worker { $segments_flag }; set { $jobs_flag } to a positive bound or { $segment_workers_flag } 1

journal.cli.think.validation.no.activity.prompts.with.activity = { $no_activity_prompts_flag } cannot be combined with { $activity_flag }

journal.cli.think.validation.reactivate.incompatible.refresh = { $reactivate_flag } is incompatible with { $refresh_flag }

journal.cli.think.validation.reactivate.requires.activity = { $reactivate_flag } requires { $activity_flag }

journal.cli.think.validation.repair.segment = { $repair_segment_flag } requires { $day_flag } and { $segment_flag } and cannot be combined with another thinking mode

journal.cli.think.validation.retry.unit = { $retry_unit_flag } requires { $day_flag } and { $retry_cause_flag } without another thinking mode

journal.cli.think.validation.segment.workers.range = { $segment_workers_flag } must be between 1 and 32

journal.cli.think.validation.segments.incompatible = { $segments_flag } is incompatible with { $segment_flag } and { $facet_flag }

journal.cli.think.validation.settle.incompatible = { $settle_flag } is incompatible with { $segment_flag }, { $segments_flag }, { $activity_flag }, { $flush_flag }, { $refresh_flag }, { $weekly_flag }, and { $cadence_flag }

journal.cli.think.validation.weekly.incompatible = { $weekly_flag } is incompatible with { $segment_flag }, { $segments_flag }, { $activity_flag }, and { $flush_flag }

journal.cli.think.validation.work.intent = { $work_intent_flag } requires a from-scratch daily request

journal.cli.think.plan.activity.count = segments: { $segments }

journal.cli.think.plan.activity.empty = No agents match activity type '{ $kind }'

journal.cli.think.plan.activity.heading = { $operation ->
    [refresh] Day { $day } { $activity_flag } { $activity } { $facet_flag } { $facet } (refresh)
    *[ordinary] Day { $day } { $activity_flag } { $activity } { $facet_flag } { $facet }
    }

journal.cli.think.plan.activity.missing = Activity not found: { $activity } in facet '{ $facet }' on { $day }

journal.cli.think.plan.activity.type = type:     { $kind }

journal.cli.think.plan.cadence = Day { $day } — cadence agents

journal.cli.think.plan.cadence.fire = { $segments ->
    [one] { $activities ->
        [one] fire  { $name } — window: { $segments } segment, { $activities } activity
        *[other] fire  { $name } — window: { $segments } segment, { $activities } activity(ies)
    }
    *[other] { $activities ->
        [one] fire  { $name } — window: { $segments } segment(s), { $activities } activity
        *[other] fire  { $name } — window: { $segments } segment(s), { $activities } activity(ies)
    }
    }

journal.cli.think.plan.cadence.idle = no-op { $name } — no new work since last cadence run

journal.cli.think.plan.cadence.skip = skip  { $name } — interval not elapsed ({ $seconds }s < { $minutes }m)

journal.cli.think.plan.day = Day { $day }

journal.cli.think.plan.day.refresh = Day { $day } (refresh)

journal.cli.think.plan.day.segment = Day { $day } segment { $segment }

journal.cli.think.plan.day.segment.refresh = Day { $day } segment { $segment } (refresh)

journal.cli.think.plan.empty.cadence = No prompts for schedule: { $cadence }

journal.cli.think.plan.empty.weekly = No prompts for schedule: { $weekly }

journal.cli.think.plan.flush = Day { $day } { $flush_flag } segment { $segment }

journal.cli.think.plan.flush.empty = No flush-eligible agents

journal.cli.think.plan.flush.talent = { $kind ->
    [generate] gen  { $name }
    *[agent] agent  { $name }
    }

journal.cli.think.plan.idle = idle segments: write stubs only (unless { $refresh_flag })

journal.cli.think.plan.orchestrator = Sense orchestrator (linear):

journal.cli.think.plan.post.indexer = Post-phase: { $command }

journal.cli.think.plan.post.stats = Post-phase: { $command }

journal.cli.think.plan.pre = Pre-phase:  { $command } { $day } { $jobs_flag } { $jobs }

journal.cli.think.plan.priority = Priority { $priority }:

journal.cli.think.plan.reprocess = { $count ->
    [one] Day { $day } — re-process { $count } segment
    *[other] Day { $day } — re-process { $count } segments
    }

journal.cli.think.plan.schedule.empty = No prompts for schedule: { $schedule }

journal.cli.think.plan.segment.row = [{ $index }/{ $total }] { $segment } ({ $start }-{ $end }) stream={ $stream }

journal.cli.think.plan.skip = skip { $name } — no activity: { $facets }

journal.cli.think.plan.state = activity state machine: updates per segment

journal.cli.think.plan.step.entities = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (exists) — always for non-idle
    [new] { $step }. { $name } ({ $kind }/{ $format }) (new) — always for non-idle
    *[plain] { $step }. { $name } ({ $kind }/{ $format }) — always for non-idle
    }

journal.cli.think.plan.step.possible = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (exists) — if a speech transcript (never gates the segment)
    [new] { $step }. { $name } ({ $kind }/{ $format }) (new) — if a speech transcript (never gates the segment)
    *[plain] { $step }. { $name } ({ $kind }/{ $format }) — if a speech transcript (never gates the segment)
    }

journal.cli.think.plan.step.screen = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (exists) — if { $recommend_screen_record }
    [new] { $step }. { $name } ({ $kind }/{ $format }) (new) — if { $recommend_screen_record }
    *[plain] { $step }. { $name } ({ $kind }/{ $format }) — if { $recommend_screen_record }
    }

journal.cli.think.plan.step.sense = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (exists) — mandatory
    [new] { $step }. { $name } ({ $kind }/{ $format }) (new) — mandatory
    *[plain] { $step }. { $name } ({ $kind }/{ $format }) — mandatory
    }

journal.cli.think.plan.step.speaker.attribution = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (exists) — if audio embeddings (the model layer only if { $recommend_speaker_attribution })
    [new] { $step }. { $name } ({ $kind }/{ $format }) (new) — if audio embeddings (the model layer only if { $recommend_speaker_attribution })
    *[plain] { $step }. { $name } ({ $kind }/{ $format }) — if audio embeddings (the model layer only if { $recommend_speaker_attribution })
    }

journal.cli.think.plan.table.facet = { $row ->
    [generator_existing] gen  { $name }/{ $facet } (exists)
    [generator_new] gen  { $name }/{ $facet } (new)
    [generator_plain] gen  { $name }/{ $facet }
    [agent_existing] agent  { $name }/{ $facet } (exists)
    [agent_new] agent  { $name }/{ $facet } (new)
    *[agent_plain] agent  { $name }/{ $facet }
    }

journal.cli.think.plan.table.talent = { $row ->
    [generator_existing] gen  { $name } (exists)
    [generator_new] gen  { $name } (new)
    [generator_plain] gen  { $name }
    [agent_existing] agent  { $name } (exists)
    [agent_new] agent  { $name } (new)
    *[agent_plain] agent  { $name }
    }

journal.cli.think.plan.total = { $total ->
    [one] Total: { $total } agent
    *[other] Total: { $total } agents
    }

journal.cli.think.plan.weekly = Day { $day } — weekly agents

journal.cli.think.sense.complete = Batch processing complete

journal.cli.think.sense.day.missing = Day directory not found: { $path }

journal.cli.think.sense.deferred = { $quantity ->
    [one] Batch processing deferred: { $deferred } of { $ran } handler waiting; unfinished files will retry
    *[other] Batch processing deferred: { $deferred } of { $ran } handlers waiting; unfinished files will retry
    }

journal.cli.think.sense.files = { $quantity ->
    [one] Found { $count } unprocessed file to process ({ $extensions })
    *[other] Found { $count } unprocessed files to process ({ $extensions })
    }

journal.cli.think.sense.nothing = No unprocessed files found in { $path }

journal.cli.think.sense.processing.day = { $quantity ->
    [one] Processing files from day { $day } with { $jobs } concurrent job
    *[other] Processing files from day { $day } with { $jobs } concurrent jobs
    }

journal.cli.think.sense.processing.segment = { $quantity ->
    [one] Processing files from day { $day } (segment: { $segment }) with { $jobs } concurrent job
    *[other] Processing files from day { $day } (segment: { $segment }) with { $jobs } concurrent jobs
    }

journal.cli.think.outcome.completed = { $command }: { $count } completed

journal.cli.think.outcome.failed = { $command }: { $count } failed

journal.cli.think.reprocess.record = could not record reprocess outcome: { $error }

journal.cli.think.activity.blocked = activity blocked

journal.cli.think.activity.body.data = this activity comes from a { $provider } workout, which is body data; nothing further runs on it.

journal.cli.think.activity.malformed = the facet declaration is malformed. correct it, then retry with { $reactivate_flag }.

journal.cli.think.activity.missing = the destination facet is missing. after restoring its declaration, retry with { $reactivate_flag }; otherwise reclassify the source segment.

journal.cli.think.activity.muted.blocked = the destination facet is muted. unmute it, then retry the blocked activity with { $reactivate_flag }.

journal.cli.think.activity.muted.retry = the destination facet is muted. unmute it to resume unfinished activity processing.

journal.cli.think.activity.owner.field.held = activity output is held by owner

journal.cli.think.activity.pending = activity retry pending

journal.cli.think.activity.read.blocked = the facet declaration could not be read. after restoring access, retry the blocked activity with { $reactivate_flag }.

journal.cli.think.activity.read.retry = the facet declaration could not be read. the unfinished activity will retry automatically.

journal.cli.think.activity.record.missing = no activity record exists for this classification. after correcting the declaration, reprocess the source segment.

journal.cli.think.activity.replaced = the destination facet was replaced. reclassify the source segment; an old result cannot be applied to the replacement.

journal.cli.think.activity.settled = { $facet }/{ $activity } settled: this activity comes from a { $provider } workout, which is body data; nothing further runs on it.

journal.cli.think.activity.routing = correct the declaration, then reprocess this source segment.

journal.cli.think.daily.blockers.more = { $names }, +{ $count } more

journal.cli.think.daily.incomplete = { $units ->
    [one] { $blockers ->
        [one] { $phase } ({ $units } daily unit non-terminal; { $blockers } segment blocker remains{ $details })
        *[other] { $phase } ({ $units } daily unit non-terminal; { $blockers } segment blocker(s) remain{ $details })
    }
    *[other] { $blockers ->
        [one] { $phase } ({ $units } daily unit(s) non-terminal; { $blockers } segment blocker remains{ $details })
        *[other] { $phase } ({ $units } daily unit(s) non-terminal; { $blockers } segment blocker(s) remain{ $details })
    }
    }

journal.cli.think.daily.incomplete.record = { $phase } ({ $units } daily unit(s) non-terminal; { $blockers } segment blocker(s) remain{ $details })

journal.cli.think.log.name.answers = name answers: { $answers } changed, { $judged } activities judged again, { $changed } changed, { $waiting } waiting

journal.cli.think.log.provider = { $left ->
    [one] thinking provider is unavailable; { $left } segment waits for the next run
    *[other] thinking provider is unavailable; { $left } segments wait for the next run
    }

journal.cli.think.log.work.discovery = work discovery: pass took { $elapsed } ms over { $inspected } of { $days } days; ranked { $handed } tasks for the queue

journal.cli.think.preflight.disk = Your journal has run out of space on its disk ({ $detail }). Repair: free up space on that disk, then start your journal again.

journal.cli.think.preflight.held = { $reason }: { $role_key }{ $role } { $pid_key }{ $pid } { $time_key }{ $started } { $generation_key }{ $generation } { $age_key }{ $age }; use the supervised path or stop that process cleanly

journal.cli.think.preflight.held.unavailable = { $reason }: { $owner }; use the supervised path or stop the current process cleanly

journal.cli.think.error.with.log = { $primary }; { $log }

journal.cli.think.plan.cadence.fire.record = fire  { $name } — window: { $segments } segment(s), { $activities } activity(ies)

journal.cli.think.plan.reprocess.record = Day { $day } — re-process { $count } segments

journal.cli.think.plan.total.record = Total: { $total } agents

journal.cli.think.retry.activity.changed = activity retry permission changed

journal.cli.think.retry.activity.released = activity is no longer set aside

journal.cli.think.retry.daily.changed = daily retry permission changed

journal.cli.think.retry.daily.missing = missing daily permission

journal.cli.think.retry.daily.released = daily unit is no longer held

journal.cli.think.retry.daily.terminal.missing = missing daily terminal record

journal.cli.think.retry.daily.unresolved = daily retry is still unresolved

journal.cli.think.retry.daily.unused = daily writer did not consume retry permission

journal.cli.think.retry.day.mismatch = retry day does not match

journal.cli.think.retry.evidence.missing = retry evidence is missing

journal.cli.think.retry.permission.changed = retry permission changed

journal.cli.think.retry.raw.released = native output is no longer held

journal.cli.think.retry.segment.changed = segment retry permission changed

journal.cli.think.activity.retry.identity = activity work identity mismatch

journal.cli.think.activity.retry.kind = not activity work

journal.cli.think.activity.retry.muted = the activity's facet is muted

journal.cli.think.log.budget = think outcome exceeds operational log record budget

journal.cli.think.log.day.format = day must be { $date_format }

journal.cli.think.log.day.local = invalid local day

journal.cli.think.log.failed = think run log { $path } failed: { $detail }

journal.cli.think.log.failure.lock = think run log failure lock poisoned

journal.cli.think.log.lock = think run log lock poisoned

journal.cli.think.log.reconciliation = retry reconciliation failed: { $error }

journal.cli.think.priority.environment = invalid work priority: { $error }

journal.cli.think.priority.syntax = invalid work priority; expected band 1–6 and a source time

journal.cli.think.cancelled = thinking cancelled

journal.cli.think.repair.segment.missing = missing repair segment

journal.cli.think.sense.error.delete = could not delete { $path }: { $error }

journal.cli.think.sense.error.deleted.dirty = reprocess removed { $path }, but could not durably mark day { $day } dirty: { $detail }

journal.cli.think.sense.error.failed = { $failed } failed of { $ran } ran

journal.cli.think.sense.error.held = sense batch lease for day { $day } is held by another process

journal.cli.think.sense.error.join = Sense worker did not stop cleanly

journal.cli.think.sense.error.lease = sense batch lease error: { $error }

journal.cli.think.sense.error.runtime = batch callosum runtime unavailable

journal.cli.think.sense.error.scan = could not scan { $path }: { $error }

journal.cli.think.sense.error.timeout = sense batch timed out after { $duration }

journal.cli.think.sense.refused = sense batch refused: { $reason }

journal.cli.think.sense.retry.changed = native retry inputs changed

journal.cli.think.sense.retry.handler = unknown native retry handler

journal.cli.think.sense.retry.kind = retry is not native analysis

journal.cli.think.sense.retry.mismatch = native failure handler does not match its source

journal.cli.think.wait.cortex = cortex wait failed: { $error }

journal.cli.think.wait.failed = wait failed: { $error }

journal.cli.think.health.day.invalid = invalid day { $day }; expected { $date_format }

journal.cli.think.health.directory = cannot read directory { $path }: { $detail }

journal.cli.think.health.marker = health marker error: { $detail }

journal.cli.think.health.marker.atomic = health marker publication failed: { $error }

journal.cli.think.health.marker.fingerprint = health marker fingerprint read failed: { $detail }

journal.cli.think.health.marker.io = health marker I/O failed at { $path }: { $error }

journal.cli.think.health.marker.lock = health marker lock failed: { $error }

journal.cli.think.health.marker.malformed = health marker is malformed: { $path }

journal.cli.think.health.metadata = cannot read file metadata { $path }: { $detail }

journal.cli.think.health.read = journal I/O error: { $detail }

journal.cli.think.health.source = health-log source error: { $detail }

journal.cli.think.phase.exit = exit { $code }

journal.cli.think.phase.segment.prerequisites = segment prerequisites changed

journal.cli.think.activity.day.missing = activity day file is missing: { $path }

journal.cli.think.activity.row.missing = activity row is missing: { $path }

journal.cli.think.health.progress.unreadable = segment progress contains unreadable records

journal.cli.think.intent.gone = destructive request is no longer waiting

journal.cli.think.retry.talent.disabled = held talent is no longer enabled

journal.cli.think.retry.talent.dispatch = held talent could not be dispatched

journal.cli.think.retry.talent.segment.missing = segment no longer exists

journal.cli.think.retry.talent.skipped = held talent could not be retried

journal.cli.think.activity.state = activity state not saved: { $error }

journal.cli.think.activity.state.segment = { $segment }: activity state not saved: { $error }

journal.cli.think.activity.work.pending = activity work pending: { $failures }

journal.cli.think.health.marker.stream = stream health marker is malformed

journal.cli.think.phase.daily.prerequisites = daily work is waiting for its segment prerequisites

journal.cli.think.settle.save = activity settle state not saved: { $error }

journal.cli.think.markdown.entities = Sense Entities

journal.cli.think.markdown.objects = Objects

journal.cli.think.markdown.partial.input = Left out: Some input did not fit. This result was written from part of the input.

journal.cli.think.markdown.partial.story = This story, and anything saved with it, was written from part of the activity, not all of it.

journal.cli.think.markdown.quotes = Quotes

journal.cli.think.markdown.speaker = Speaker { $number }

journal.cli.think.markdown.terminal.clock = { $clock } · terminal

journal.cli.think.markdown.terminal.notice = text shown in your terminal. this says what was visible, not what you read, typed or did. masking passwords and other secrets is best effort; your redaction rules do not apply.

journal.cli.think.markdown.topics = Topics

journal.cli.think.markdown.object.both = { $frames ->
    [one] { $photos ->
        [one] { $class }: in { $frames } frame and { $photos } photo
        *[other] { $class }: in { $frames } frame and { $photos } photos
    }
    *[other] { $photos ->
        [one] { $class }: in { $frames } frames and { $photos } photo
        *[other] { $class }: in { $frames } frames and { $photos } photos
    }
    }

journal.cli.think.markdown.object.both.maximum = { $frames ->
    [one] { $photos ->
        [one] { $class }: in { $frames } frame and { $photos } photo, up to { $most } at once
        *[other] { $class }: in { $frames } frame and { $photos } photos, up to { $most } at once
    }
    *[other] { $photos ->
        [one] { $class }: in { $frames } frames and { $photos } photo, up to { $most } at once
        *[other] { $class }: in { $frames } frames and { $photos } photos, up to { $most } at once
    }
    }

journal.cli.think.markdown.object.frames = { $frames ->
    [one] { $class }: in { $frames } frame
    *[other] { $class }: in { $frames } frames
    }

journal.cli.think.markdown.object.frames.maximum = { $frames ->
    [one] { $class }: in { $frames } frame, up to { $most } at once
    *[other] { $class }: in { $frames } frames, up to { $most } at once
    }

journal.cli.think.markdown.object.photos = { $photos ->
    [one] { $class }: in { $photos } photo
    *[other] { $class }: in { $photos } photos
    }

journal.cli.think.markdown.object.photos.maximum = { $photos ->
    [one] { $class }: in { $photos } photo, up to { $most } at once
    *[other] { $class }: in { $photos } photos, up to { $most } at once
    }

journal.cli.think.markdown.partial.counts = Left out: { $details }

journal.cli.think.markdown.entity = { $kind } — { $name } (role={ $role }, source={ $source }) — { $context }

journal.cli.think.markdown.sense.entities = ### Entities

journal.cli.think.markdown.sense.facets = ### Facets

journal.cli.think.markdown.sense.record = ## Sense: { $kind }

journal.cli.think.markdown.sense.speakers = **Speakers:** { $names }

journal.cli.think.retry.raw.coordinate.invalid = invalid native retry coordinate

journal.cli.think.retry.raw.coordinate.utf8 = non-UTF8 retry coordinate

journal.cli.think.retry.raw.fingerprint.invalid = native failure input fingerprint is malformed

journal.cli.think.retry.raw.handler.missing = native failure has no handler

journal.cli.think.retry.raw.header.incomplete = native processing header is incomplete

journal.cli.think.retry.raw.header.kind = native processing header is not an object

journal.cli.think.retry.raw.kind = not native analysis

journal.cli.think.retry.raw.parent.missing = native output has no parent

journal.cli.think.retry.raw.permission.unused = native writer did not consume the retry permission

journal.cli.think.retry.raw.receipt.changed = native terminal receipt changed

journal.cli.think.retry.raw.receipt.missing = native terminal receipt is missing

journal.cli.think.retry.raw.receipt.unpublished = native writer has not published this retry's terminal receipt

journal.cli.think.retry.raw.receipt.unreadable = native terminal receipt is unreadable

journal.cli.think.retry.raw.record.malformed = native processing record is malformed

journal.cli.think.retry.raw.schema.unknown = native processing schema is unrecognized

journal.cli.think.retry.raw.source.invalid = native failure has no safe source filename

journal.cli.think.retry.raw.source.mismatch = native failure source does not match its output

journal.cli.think.activity.fingerprint.failed = cannot fingerprint activity inputs

journal.cli.think.activity.repair.required = requires repair

journal.cli.think.activity.work.malformed = malformed activity work: { $path }

journal.cli.think.activity.work.running = previous activity input is still running

journal.cli.think.activity.work.unreadable = cannot read activity work { $path }: { $error }

journal.cli.think.activity.work.version = unsupported activity work version

journal.cli.think.daily.inputs.changed = raw input changed during whole-day processing

journal.cli.think.daily.location.packet = daily location reuse has no stored packet

journal.cli.think.daily.location.read = daily location could not be read

journal.cli.think.daily.snapshot.changed = daily evidence changed before preparation

journal.cli.think.daily.snapshot.changed.after = daily evidence or contract changed during preparation

journal.cli.think.daily.snapshot.freeze = daily input preparation: { $error }

journal.cli.think.daily.snapshot.parent = talent root has no parent

journal.cli.think.daily.snapshot.removed = daily talent removed during preparation

journal.cli.think.routing.log.malformed = cannot read routing recovery from malformed health logs

journal.cli.think.routing.source.invalid = retained routing source is invalid

journal.cli.think.segment.ambiguous = { $count ->
    [one] ambiguous segment { $day }/{ $segment }: { $count } stream shares this name
   *[other] ambiguous segment { $day }/{ $segment }: { $count } streams share this name
    }

journal.cli.think.sense.config.concurrency = sense: invalid { $handler }.{ $setting }; defaulting to 1

journal.cli.think.sense.config.read = sense: journal config unavailable: { $error }

journal.cli.think.sense.config.runtime = sense: invalid { $handler }.{ $setting }; defaulting to { $seconds }s

journal.cli.think.sense.priority.encoding = { $variable } contains invalid UTF-8

journal.cli.think.sense.worker.authority = { $handler } launch authority was not managed

journal.cli.think.sense.worker.deferred = deferred (exit { $code })

journal.cli.think.sense.worker.detail = sense: day={ $day } stream={ $stream } segment={ $segment } file={ $file } error={ $error }

journal.cli.think.sense.worker.exit = { $handler } exit { $code }

journal.cli.think.sense.worker.failed = failed (exit { $code })

journal.cli.think.sense.worker.launch = { $handler } spawn failed

journal.cli.think.sense.worker.log.tail = sense: handler diagnostic log={ $log } tail={ $tail }

journal.cli.think.sense.worker.log.unreadable = sense: handler diagnostic log={ $log } unreadable: { $error }

journal.cli.think.sense.worker.output.incomplete = { $handler } exited 0 with unfinished output (analysis output incomplete): { $path }

journal.cli.think.sense.worker.output.nonregular = { $handler } exited 0 without a regular output: { $path }

journal.cli.think.sense.worker.output.unfinished = { $handler } exited 0 with unfinished output ({ $reason }): { $path }

journal.cli.think.sense.worker.output.unreadable = { $handler } exited 0 without readable output { $path }: { $error }

journal.cli.think.sense.worker.pool = { $handler } pool unavailable

journal.cli.think.sense.worker.program.executable = { $handler } dispatcher current_exe failed: { $error }

journal.cli.think.sense.worker.program.missing = { $handler } dispatcher missing: { $path }

journal.cli.think.sense.worker.program.nonexecutable = { $handler } dispatcher not executable: { $path }

journal.cli.think.sense.worker.program.nonregular = { $handler } dispatcher not a file: { $path }

journal.cli.think.sense.worker.status = sense: handler={ $handler } file={ $file } outcome={ $outcome } log={ $log }

journal.cli.think.sense.worker.survivors = sense: process-tree-not-reaped handler={ $handler } reason={ $reason } survivors={ $count }

journal.cli.think.sense.worker.termination = sense: managed termination failed handler={ $handler }: { $error }

journal.cli.think.sense.worker.timeout = { $handler } watchdog_timeout after { $seconds }s

journal.cli.think.sense.worker.watchdog = watchdog or process wait failed

journal.cli.think.updated.capability = catchup capability unavailable on this platform

journal.cli.think.updated.io = catchup I/O failed at { $path }: { $error }

journal.cli.think.updated.marker = catchup marker error: { $error }

journal.cli.think.updated.path = catchup journal path error: { $error }

journal.cli.think.updated.state = catchup state is malformed: { $detail }

journal.cli.think.sense.interrupted.write = failed to record { $event } event to { $path }: { $error }

journal.cli.think.sense.lease.skip = Sense batch lease for day { $day } is held; skipping batch walk

journal.cli.think.sense.worker.launch.failed = sense: { $handler } launch failed: { $error }

journal.cli.think.log.activity.join = { $command }: late { $day }/{ $segment } joined its open activity

journal.cli.think.log.activity.no.advance = { $command }: { $day }/{ $segment } does not advance live activity state

journal.cli.think.log.activity.replay = { $command }: late { $day }/{ $segment } is replayed with its stream's day

journal.cli.think.log.activity.turn = advancing { $stream } activity state without its turn: { $error }

journal.cli.think.log.cadence.read = Failed to load cadence state: { $error }

journal.cli.think.log.cadence.set.aside = Failed to load cadence state: corrupt artifact set aside

journal.cli.think.log.daily.search = could not inspect search work before daily thinking: { $error }

journal.cli.think.log.display.metadata = could not save display metadata for { $path }: { $error }

journal.cli.think.log.held.release = could not release held { $name } for { $segment }: { $error }

journal.cli.think.log.segment.turn = thinking { $day }/{ $segment } without its turn: { $error }

journal.cli.think.log.provider.record = thinking provider is unavailable; { $left } segments wait for the next run

journal.cli.transcribe.batch.day = { $count ->
    [one] { $verb }: { $count } day could not be read
   *[other] { $verb }: { $count } day(s) could not be read
    }

journal.cli.transcribe.batch.day.record = { $verb }: { $count } day(s) could not be read

journal.cli.transcribe.batch.failed = transcription: { $failed } failed

journal.cli.transcribe.config.backend = invalid { $key } in journal config; choose one of: { $backends }

journal.cli.transcribe.file.failed = { $file }: transcription failed ({ $reason }); details: { $output }

journal.cli.transcribe.owner.running = your journal is running and transcribes recordings itself. bring the recording in with '{ $import }' instead.

journal.cli.transcribe.owner.stopped = your journal isn't running. start it with '{ $up }', then bring the recording in with '{ $import }'; your journal transcribes it.

journal.cli.transcribe.summary = { $processed } processed, { $skipped } skipped (already transcribed)

journal.cli.transcribe.summary.deferred = { $processed } processed, { $skipped } skipped (already transcribed), { $deferred } deferred (provider not ready, will retry)

journal.cli.transcribe.summary.deferred.failed = { $processed } processed, { $skipped } skipped (already transcribed), { $deferred } deferred (provider not ready, will retry), { $failed } failed

journal.cli.transcribe.summary.failed = { $processed } processed, { $skipped } skipped (already transcribed), { $failed } failed

journal.cli.transcribe.feed.failure = speaker candidate pool feed failed for { $path }: { $detail }

journal.cli.transcribe.feed.parse = failed to parse transcript: { $detail }

journal.cli.transcribe.feed.pool.lock = candidate pool lock failed: { $detail }

journal.cli.transcribe.feed.pool.read = candidate pool read failed: { $detail }

journal.cli.transcribe.feed.pool.width = candidate pool centroid width mismatch

journal.cli.transcribe.feed.pool.write = candidate pool write failed: { $detail }

journal.cli.transcribe.feed.read = failed to read transcript: { $detail }

journal.cli.transcribe.feed.unreadable = completed transcript or embeddings could not be read

journal.cli.transcribe.audio.directory = Audio file must be in a segment directory ({ $pattern }), but parent is: { $parent }

journal.cli.transcribe.audio.format = Unsupported audio format: { $extension }. Supported formats: { $formats }

journal.cli.transcribe.audio.missing = Audio file not found.{ $line_break }  Tried absolute:         { $absolute }{ $line_break }  Tried journal-relative: { $relative }

journal.cli.transcribe.backend.choice = argument { $backend_flag }: invalid choice: '{ $backend }' (choose from { $choices })

journal.cli.transcribe.backend.duplicate = argument { $backend_flag }: may only be specified once

journal.cli.transcribe.claim.input.cancelled = processing input verification cancelled

journal.cli.transcribe.claim.input.changed.commit = processing input changed before publication

journal.cli.transcribe.claim.input.changed.open = processing input changed while opening

journal.cli.transcribe.claim.input.changed.read = processing input changed while reading

journal.cli.transcribe.claim.input.no.attempt = input has no output attempt

journal.cli.transcribe.claim.input.not.file = processing input is not a plain file

journal.cli.transcribe.claim.lock.entry.unsafe = persistent lock entry is an unsafe { $kind }: { $path }

journal.cli.transcribe.claim.lock.invalid = invalid persistent lock entry { $name }

journal.cli.transcribe.claim.lock.missing = persistent lock parent is missing: { $path }

journal.cli.transcribe.claim.lock.mode = persistent lock entry has mode { $mode }, expected 600: { $path }

journal.cli.transcribe.claim.lock.namespace = persistent lock entry changed during acquisition: { $path }

journal.cli.transcribe.claim.lock.parent.changed = persistent lock parent changed during acquisition: { $path }

journal.cli.transcribe.claim.lock.parent.unsafe = persistent lock parent is an unsafe { $kind }: { $path }

journal.cli.transcribe.claim.parent.missing = missing parent directory

journal.cli.transcribe.claim.root.absent = journal root does not exist

journal.cli.transcribe.claim.root.absolute = journal root must be absolute

journal.cli.transcribe.claim.root.ancestor = canonical journal root has a non-UTF-8 ancestor

journal.cli.transcribe.claim.root.canonical.absolute = canonical journal root is not absolute

journal.cli.transcribe.claim.root.changed = journal source changed during acquisition

journal.cli.transcribe.claim.root.component = canonical journal root has no final component

journal.cli.transcribe.claim.root.directory = journal root is not a directory

journal.cli.transcribe.claim.root.invalid = invalid journal root { $path }: { $detail }

journal.cli.transcribe.claim.root.io = { $operation }: { $reason }

journal.cli.transcribe.claim.root.unsupported = unsupported journal root { $path }: { $detail }

journal.cli.transcribe.claim.segment.not.covered = segment is not covered by these output attempts

journal.cli.transcribe.claim.target.name = missing lock target name

journal.cli.transcribe.error.asset.absent = could not find nonempty model asset { $asset }; searched { $paths }

journal.cli.transcribe.error.asset.digest = model asset { $asset } at { $path } has { $code_sha256 } { $actual }, expected { $expected }

journal.cli.transcribe.error.asset.executable = could not determine current executable: { $error }

journal.cli.transcribe.error.asset.override = { $code_solstone_transcribe_model_assets_dir }={ $directory } does not provide nonempty model asset { $asset }

journal.cli.transcribe.error.audio.budget = decoded sample budget { $limit } exceeded for { $path }

journal.cli.transcribe.error.audio.corrupt = corrupt audio input { $path }: { $detail }

journal.cli.transcribe.error.audio.empty = audio input is empty: { $path }

journal.cli.transcribe.error.audio.ffmpeg = { $code_ffmpeg } error for { $path }: { $detail }

journal.cli.transcribe.error.audio.flush = { $iterations ->
    [one] { $flushed ->
        [one] resampler flush did not converge for { $path } after { $iterations } iteration ({ $flushed } sample flushed; { $remaining } still delayed)
        *[other] resampler flush did not converge for { $path } after { $iterations } iteration ({ $flushed } samples flushed; { $remaining } still delayed)
    }
    *[other] { $flushed ->
        [one] resampler flush did not converge for { $path } after { $iterations } iterations ({ $flushed } sample flushed; { $remaining } still delayed)
        *[other] resampler flush did not converge for { $path } after { $iterations } iterations ({ $flushed } samples flushed; { $remaining } still delayed)
    }
    }

journal.cli.transcribe.error.audio.io = { $code_i_o } error for { $path }: { $error }

journal.cli.transcribe.error.audio.sample.rate = invalid { $code_wav } sample rate: { $rate }

journal.cli.transcribe.error.audio.samples = no audio samples decoded from { $path }

journal.cli.transcribe.error.audio.sidecar.create = cannot create { $code_f32le } sidecar { $path }: { $error }

journal.cli.transcribe.error.audio.sidecar.sync = cannot sync { $code_f32le } sidecar { $path }: { $error }

journal.cli.transcribe.error.audio.sidecar.write = cannot write { $code_f32le } sidecar { $path }: { $error }

journal.cli.transcribe.error.audio.stream = no audio stream in { $path }

journal.cli.transcribe.error.audio.wav.size = { $samples ->
    [one] WAV data is too large for a { $riff_u32 } field: { $samples } sample
   *[other] WAV data is too large for a { $riff_u32 } field: { $samples } samples
    }

journal.cli.transcribe.error.backend.memory = no viable { $code_stt } backend (available memory: { $available }, local floor: { $floor })

journal.cli.transcribe.error.backend.platform = { $code_stt } backend is unavailable on this host platform: { $backend }

journal.cli.transcribe.error.cancelled = embedded transcription cancelled before publication

journal.cli.transcribe.error.claim = could not acquire transcription ownership for { $path }: { $detail }

journal.cli.transcribe.error.confidential.confidential.audio.handling.is.disabled = confidential audio handling is disabled

journal.cli.transcribe.error.confidential.confidential.lane.blocks.stt.backend.backend.raw.audio = confidential lane blocks { $code_stt } backend { $backend }; raw audio must stay local

journal.cli.transcribe.error.confidential.failure = confidential transcription { $reason }: { $detail }

journal.cli.transcribe.error.confidential.hosted.stt.returned.http.status = hosted { $code_stt } returned { $code_http } { $status }

journal.cli.transcribe.error.confidential.hosted.stt.returned.http.unauthorized = hosted { $code_stt } returned { $code_http } 401

journal.cli.transcribe.error.confidential.the.confidential.admission.probe.failed = the confidential admission probe failed

journal.cli.transcribe.error.confidential.the.confidential.attestation.channel.is.not.ready = the confidential attestation channel is not ready

journal.cli.transcribe.error.confidential.the.confidential.endpoint.has.no.credential = the confidential endpoint has no credential

journal.cli.transcribe.error.confidential.the.confidential.lane.has.no.confidential.byo.endpoint = the confidential lane has no confidential { $code_byo } endpoint

journal.cli.transcribe.error.confidential.the.confidential.lane.is.no.longer.active = the confidential lane is no longer active

journal.cli.transcribe.error.confidential.word.a.word.entry.was.blank = a word entry was blank

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.a.required.key = a word entry was missing a required key

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.end = a word entry was missing `end`

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.start = a word entry was missing `start`

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.word = a word entry was missing `word`

journal.cli.transcribe.error.confidential.word.a.word.entry.was.not.an.object = a word entry was not an object

journal.cli.transcribe.error.confidential.word.contract = hosted { $code_stt } response violated the verbose { $code_json } contract: { $detail }

journal.cli.transcribe.error.confidential.word.response.had.no.words.array = response had no words array

journal.cli.transcribe.error.confidential.word.response.had.text.but.no.word.timings = response had text but no word timings

journal.cli.transcribe.error.confidential.word.response.was.not.an.object = response was not an object

journal.cli.transcribe.error.confidential.word.response.was.not.json = response was not { $code_json }

journal.cli.transcribe.error.confidential.word.word.key.was.not.a.finite.number.found = word `{ $key }` was not a finite number (found { $found })

journal.cli.transcribe.error.coreml.failure = { $code_parakeet } { $code_coreml } { $reason }: { $detail }

journal.cli.transcribe.error.coreml.helper.json.was.not.an.object = helper { $code_json } was not an object

journal.cli.transcribe.error.coreml.helper.response.has.invalid.key = helper response has invalid { $key }

journal.cli.transcribe.error.coreml.helper.response.missing.key = helper response missing { $key }

journal.cli.transcribe.error.coreml.helper.response.missing.token.timings = helper response missing { $code_token_timings }

journal.cli.transcribe.error.coreml.helper.returned.transcript.text.without.token.timings = helper returned transcript text without token timings

journal.cli.transcribe.error.coreml.helper.token.timings.collapsed.to.no.words = helper token timings collapsed to no words

journal.cli.transcribe.error.coreml.helper.version.json.was.not.an.object = helper version { $code_json } was not an object

journal.cli.transcribe.error.coreml.helper.version.response.missing.field = helper version response missing { $field }

journal.cli.transcribe.error.coreml.missing.stderr.pipe = missing { $code_stderr } pipe

journal.cli.transcribe.error.coreml.missing.stdout.pipe = missing { $code_stdout } pipe

journal.cli.transcribe.error.coreml.name.reader.panicked = { $name } reader panicked

journal.cli.transcribe.error.coreml.parakeet.coreml.model.is.not.installed.yet.path = { $code_parakeet } { $code_coreml } model is not installed yet: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.exited.with.status = { $code_parakeet } helper exited with { $status }

journal.cli.transcribe.error.coreml.parakeet.helper.is.missing.path = { $code_parakeet } helper is missing: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.is.not.a.file.path = { $code_parakeet } helper is not a file: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.is.not.executable.path = { $code_parakeet } helper is not executable: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.timed.out = { $code_parakeet } helper timed out

journal.cli.transcribe.error.coreml.parakeet.helper.timed.out.after.seconds.s = { $code_parakeet } helper timed out after { $seconds }s

journal.cli.transcribe.error.coreml.publication.busy = { $code_parakeet } { $code_coreml } publication lock busy

journal.cli.transcribe.error.coreml.token.timing.must.be.an.object = token timing must be an object

journal.cli.transcribe.error.cpp.device.must.be.one.of.auto.cpu = device must be one of: { $code_auto_cpu }

journal.cli.transcribe.error.cpp.failure = { $code_parakeet_cpp } { $reason }: { $detail }

journal.cli.transcribe.error.cpp.http.status = { $code_http } { $status }

journal.cli.transcribe.error.cpp.parakeet.cpp.is.only.supported.on.linux.and = { $code_parakeet_cpp } is only supported on linux and windows

journal.cli.transcribe.error.cpp.parakeet.server.credentials.are.unavailable.in.this.process = { $code_parakeet } server credentials are unavailable in this process.

journal.cli.transcribe.error.cpp.parakeet.server.is.not.ready.yet = { $code_parakeet } server is not ready yet.

journal.cli.transcribe.error.cpp.response.has.text.but.no.word.timings = response has text but no word timings

journal.cli.transcribe.error.cpp.response.json.was.invalid.error = response { $code_json } was invalid: { $error }

journal.cli.transcribe.error.cpp.response.json.was.not.an.object = response { $code_json } was not an object

journal.cli.transcribe.error.cpp.response.missing.top.level.words = response missing top-level { $code_words }

journal.cli.transcribe.error.cpp.tls.is.required = { $code_tls } is required

journal.cli.transcribe.error.cpp.transcription.request.timed.out = transcription request timed out

journal.cli.transcribe.error.cpp.word.timing.contains.invalid.numeric.value = word timing contains invalid numeric value

journal.cli.transcribe.error.cpp.word.timing.item.must.be.an.object = word timing item must be an object

journal.cli.transcribe.error.cpp.word.timing.missing.key.key = word timing missing key: { $key }

journal.cli.transcribe.error.cpp.word.timing.text.was.blank = word timing text was blank

journal.cli.transcribe.error.input.inspect = could not inspect input { $path }: { $detail }

journal.cli.transcribe.error.orphan = could not remove orphan { $code_npz } { $path }: { $detail }

journal.cli.transcribe.error.payload.parent = transcript destination has no parent

journal.cli.transcribe.error.process.admission = hosted launch admission failed: { $reason }

journal.cli.transcribe.error.process.capability = host capability unavailable: { $needed }

journal.cli.transcribe.error.process.command = cannot spawn an empty command

journal.cli.transcribe.error.process.confirm = post-spawn confirmation failed for pid { $pid }: { $error }

journal.cli.transcribe.error.process.explicit = this authority is not explicitly unowned

journal.cli.transcribe.error.process.identity = failed to capture birth-bound identity for spawned pid { $pid }

journal.cli.transcribe.error.process.log = failed to prepare operational log: { $error }

journal.cli.transcribe.error.process.output = child output is unavailable

journal.cli.transcribe.error.process.spawn = failed to spawn child: { $error }

journal.cli.transcribe.error.process.terminate = failed to terminate child: { $error }

journal.cli.transcribe.error.process.unowned.reason = { $code_explicitlyunowned } reason must be nonempty

journal.cli.transcribe.error.publish.output = { $code_output_jsonl_path } is required

journal.cli.transcribe.error.publish.parent = output has no parent

journal.cli.transcribe.error.publish.raw = { $code_header_raw } is required

journal.cli.transcribe.error.publish.sibling = processing raw must be a sibling filename

journal.cli.transcribe.error.speaker.analysis = speaker analysis failed: { $stage }/{ $reason }

journal.cli.transcribe.error.statement.field = full transcript statement has no valid { $field }

journal.cli.transcribe.error.terminal.payload = could not prepare terminal payload: { $detail }

journal.cli.transcribe.error.terminal.payload.path = could not prepare or remove terminal payload { $path }: { $detail }

journal.cli.transcribe.error.terminal.request = could not serialize terminal writer request: { $detail }

journal.cli.transcribe.error.transcript.payload = could not prepare transcript payload: { $detail }

journal.cli.transcribe.error.transcript.payload.path = could not prepare or remove transcript payload { $path }: { $detail }

journal.cli.transcribe.error.transcript.request = could not serialize transcript writer request: { $detail }

journal.cli.transcribe.error.vad.contract.exit = { $code_vad } helper contract error (exit { $code }): { $detail }

journal.cli.transcribe.error.vad.contract.exit.stderr = { $code_vad } helper contract error (exit { $code }): { $detail }: { $stderr }

journal.cli.transcribe.error.vad.contract.signal = { $code_vad } helper contract error (no exit code): { $detail }

journal.cli.transcribe.error.vad.contract.signal.stderr = { $code_vad } helper contract error (no exit code): { $detail }: { $stderr }

journal.cli.transcribe.error.vad.could.not.create.vad.input.directory.error = could not create { $code_vad } input directory: { $error }

journal.cli.transcribe.error.vad.could.not.remove.vad.input.directory.error = could not remove { $code_vad } input directory: { $error }

journal.cli.transcribe.error.vad.could.not.remove.vad.input.directory.input.remains.in = could not remove { $code_vad } input directory: input remains in use

journal.cli.transcribe.error.vad.could.not.send.vad.request.error = could not send { $code_vad } request: { $error }

journal.cli.transcribe.error.vad.could.not.serialize.vad.request.error = could not serialize { $code_vad } request: { $error }

journal.cli.transcribe.error.vad.error.empty = { $code_vad } error response was empty

journal.cli.transcribe.error.vad.error.many = { $code_vad } error response contained more than one line

journal.cli.transcribe.error.vad.error.utf8 = { $code_vad } error response was not { $code_utf_8 }: { $error }

journal.cli.transcribe.error.vad.helper = { $code_vad } helper { $reason } (exit { $code }): { $detail }

journal.cli.transcribe.error.vad.launch.failed = could not launch { $code_vad } helper { $path }: { $detail }

journal.cli.transcribe.error.vad.response.empty = { $code_vad } response was empty

journal.cli.transcribe.error.vad.response.many = { $code_vad } response contained more than one line

journal.cli.transcribe.error.vad.response.utf8 = { $code_vad } response was not { $code_utf_8 }: { $error }

journal.cli.transcribe.error.vad.vad.error.response.lacks.schema.reason.or.detail = { $code_vad } error response lacks schema, reason, or detail

journal.cli.transcribe.error.vad.vad.error.response.must.be.an.object = { $code_vad } error response must be an object

journal.cli.transcribe.error.vad.vad.error.response.violates.its.schema = { $code_vad } error response violates its schema

journal.cli.transcribe.error.vad.vad.error.response.was.not.json.error = { $code_vad } error response was not { $code_json }: { $error }

journal.cli.transcribe.error.vad.vad.helper.stdin.was.unavailable = { $code_vad } helper { $code_stdin } was unavailable

journal.cli.transcribe.error.vad.vad.helper.terminated.without.an.exit.code = { $code_vad } helper terminated without an exit code

journal.cli.transcribe.error.vad.vad.response.has.an.unknown.schema = { $code_vad } response has an unknown schema

journal.cli.transcribe.error.vad.vad.response.has.no.boolean.has.speech = { $code_vad } response has no boolean { $code_has_speech }

journal.cli.transcribe.error.vad.vad.response.has.no.non.empty.field = { $code_vad } response has no non-empty { $field }

journal.cli.transcribe.error.vad.vad.response.has.no.non.negative.finite.field = { $code_vad } response has no non-negative finite { $field }

journal.cli.transcribe.error.vad.vad.response.has.no.speech.array = { $code_vad } response has no speech array

journal.cli.transcribe.error.vad.vad.response.must.be.an.object = { $code_vad } response must be an object

journal.cli.transcribe.error.vad.vad.response.was.not.json.error = { $code_vad } response was not { $code_json }: { $error }

journal.cli.transcribe.error.vad.vad.speech.entry.ends.before.it.starts = { $code_vad } speech entry ends before it starts

journal.cli.transcribe.error.vad.vad.speech.entry.has.no.valid.field.sample.index = { $code_vad } speech entry has no valid { $field } sample index

journal.cli.transcribe.error.vad.vad.speech.entry.must.be.an.object = { $code_vad } speech entry must be an object

journal.cli.transcribe.error.vad.wait.failed = could not wait for { $code_vad } helper: { $detail }

journal.cli.transcribe.error.vad.write = could not write { $code_vad } input: { $detail }

journal.cli.transcribe.error.voice.wait = voice-only transcription is waiting: { $detail }

journal.cli.transcribe.error.windows.admission.deadline.exceeded = bounded helper admission did not complete before its deadline

journal.cli.transcribe.error.windows.blocked = { $detail }; prior helper cleanup blocks admission: { $blockers }

journal.cli.transcribe.error.windows.cancelled = bounded helper was cancelled

journal.cli.transcribe.error.windows.cleanup = { $detail }; cleanup was incomplete at return: { $cleanup }

journal.cli.transcribe.error.windows.cleanup.blocked = { $detail }; cleanup was incomplete at return: { $cleanup }; prior helper cleanup blocks admission: { $blockers }

journal.cli.transcribe.error.windows.cleanup.contended = bounded helper cleanup admission is contended

journal.cli.transcribe.error.windows.cleanup.pending = earlier bounded helper cleanup blocks admission

journal.cli.transcribe.error.windows.current.directory.not.directory = bounded helper current directory is not a directory

journal.cli.transcribe.error.windows.current.directory.outside.package = bounded helper current directory is outside its package root

journal.cli.transcribe.error.windows.current.directory.unavailable = bounded helper current directory could not be canonicalized

journal.cli.transcribe.error.windows.deadline.exceeded = bounded helper did not complete before its deadline

journal.cli.transcribe.error.windows.executable.not.file = bounded helper executable is not a regular file

journal.cli.transcribe.error.windows.executable.outside.package = bounded helper executable is outside its package root

journal.cli.transcribe.error.windows.executable.unavailable = bounded helper executable could not be canonicalized

journal.cli.transcribe.error.windows.input.limit.exceeded = bounded helper input exceeds its declared byte limit

journal.cli.transcribe.error.windows.input.write.failed = bounded helper input writer failed

journal.cli.transcribe.error.windows.io.worker.start.failed = bounded helper { $stream } { $code_i_o } worker could not start

journal.cli.transcribe.error.windows.job.not.quiescent = bounded helper completion did not establish Job quiescence

journal.cli.transcribe.error.windows.launch.failed = bounded helper failed before an owned child identity was available

journal.cli.transcribe.error.windows.launch.finalization.failed = native launch finalization failed: { $detail }

journal.cli.transcribe.error.windows.missing.system.root = bounded helper environment must contain a nonempty { $code_systemroot }

journal.cli.transcribe.error.windows.output.limit.exceeded = bounded helper { $stream } output exceeded its declared byte limit

journal.cli.transcribe.error.windows.output.read.failed = bounded helper { $stream } output reader failed

journal.cli.transcribe.error.windows.package.root.unavailable = bounded helper package root could not be canonicalized

journal.cli.transcribe.error.windows.path.environment.refused = bounded helper environment may not contain { $code_path }

journal.cli.transcribe.error.windows.path.not.representable = bounded helper path cannot be represented by the managed Windows command boundary

journal.cli.transcribe.error.windows.process.observation.failed = bounded helper process observation failed

journal.cli.transcribe.error.windows.zero.limit = bounded helper { $stream } byte limit must be nonzero

journal.cli.transcribe.error.windows.zero.timeout = bounded helper timeout must be nonzero

journal.cli.transcribe.error.writer.base.time.us.of.day.is.outside.a.day = { $code_base_time_us_of_day } is outside a day

journal.cli.transcribe.error.writer.byte.count.does.not.match.shape = { $code_byte_count } does not match shape

journal.cli.transcribe.error.writer.byte.count.is.too.large = { $code_byte_count } is too large

journal.cli.transcribe.error.writer.cancelled = transcript publication was cancelled before admission

journal.cli.transcribe.error.writer.destination = destination already exists: { $path }

journal.cli.transcribe.error.writer.dtype.must.be.float32.le = { $code_dtype } must be { $code_float32_le }

journal.cli.transcribe.error.writer.duration.and.noisy.must.be.provided.together = duration and noisy must be provided together

journal.cli.transcribe.error.writer.durations.s.count.does.not.match.shape = { $code_durations_s } count does not match shape

journal.cli.transcribe.error.writer.durations.s.must.be.finite.f32.values = { $code_durations_s } must be finite { $code_f32 } values

journal.cli.transcribe.error.writer.encoder.must.be.nonempty = encoder must be nonempty

journal.cli.transcribe.error.writer.field.must.be.a.boolean = { $field } must be a boolean

journal.cli.transcribe.error.writer.field.must.be.a.finite.number = { $field } must be a finite number

journal.cli.transcribe.error.writer.field.must.be.a.string = { $field } must be a string

journal.cli.transcribe.error.writer.field.must.be.an.array = { $field } must be an array

journal.cli.transcribe.error.writer.field.must.be.an.integer = { $field } must be an integer

journal.cli.transcribe.error.writer.field.must.be.an.unsigned.integer = { $field } must be an unsigned integer

journal.cli.transcribe.error.writer.header.numbers.must.be.finite = header numbers must be finite

journal.cli.transcribe.error.writer.id.range = id must be in { $code_1_i32_max }

journal.cli.transcribe.error.writer.invalid.f32.payload.chunk = invalid { $code_f32 } payload chunk

journal.cli.transcribe.error.writer.jsonl.path.and.npz.path.must.be.sibling.files = { $code_jsonl_path } and { $code_npz_path } must be sibling files sharing a stem

journal.cli.transcribe.error.writer.missing.required.field.key = missing required field { $key }

journal.cli.transcribe.error.writer.noisy.s.is.required.when.noisy.rms.is.present = { $code_noisy_s } is required when { $code_noisy_rms } is present

journal.cli.transcribe.error.writer.npz.cancelled = cancelled

journal.cli.transcribe.error.writer.npz.count = { $code_npz } has an unexpected member count

journal.cli.transcribe.error.writer.npz.members = { $code_npz } members do not match the transcript sidecar contract

journal.cli.transcribe.error.writer.npz.missing = { $code_npz } member { $name } is missing

journal.cli.transcribe.error.writer.npz.parent = npz destination has no parent

journal.cli.transcribe.error.writer.npz.readback = { $code_npz } member { $name } did not round-trip

journal.cli.transcribe.error.writer.output.path.has.no.parent = output path has no parent

journal.cli.transcribe.error.writer.output.paths.must.be.nonempty = output paths must be nonempty

journal.cli.transcribe.error.writer.payload.finite = embedding payload is non-finite at row { $row }, column { $column }

journal.cli.transcribe.error.writer.payload.format.must.be.raw.f32le.row.major.v1 = { $code_payload_format } must be { $code_raw_f32le_row_major_v1 }

journal.cli.transcribe.error.writer.payload.size.does.not.match.byte.count = payload size does not match { $code_byte_count }

journal.cli.transcribe.error.writer.prior.jsonl.snapshot.is.missing = prior jsonl snapshot is missing

journal.cli.transcribe.error.writer.prior.npz.snapshot.is.missing = prior npz snapshot is missing

journal.cli.transcribe.error.writer.prior.restore.refused = prior restore refused

journal.cli.transcribe.error.writer.private.stage.is.missing = private stage is missing

journal.cli.transcribe.error.writer.private.transcript.copy.did.not.match.the.staged.file = private transcript copy did not match the staged file

journal.cli.transcribe.error.writer.private.transcript.jsonl.is.missing = private transcript jsonl is missing

journal.cli.transcribe.error.writer.rounded.header.number.is.not.json.representable = rounded header number is not { $code_json }-representable

journal.cli.transcribe.error.writer.schema = unknown schema: { $schema }

journal.cli.transcribe.error.writer.segment.meta.must.be.an.object = { $code_segment_meta } must be an object

journal.cli.transcribe.error.writer.shape.byte.count.overflows = shape byte count overflows

journal.cli.transcribe.error.writer.shape.must.be.row.count.width = shape must be { $code_row_count_256 }

journal.cli.transcribe.error.writer.shape.row.count.is.too.large = shape row count is too large

journal.cli.transcribe.error.writer.speaker.evidence.multi.fraction.is.required.with.speaker.evidence = { $code_speaker_evidence_multi_fraction } is required with { $code_speaker_evidence }

journal.cli.transcribe.error.writer.speaker.evidence.version.is.required.with.speaker.evidence = { $code_speaker_evidence_version } is required with { $code_speaker_evidence }

journal.cli.transcribe.error.writer.speaker.must.be.a.string.integer.null.or.absent = speaker must be a string, integer, null, or absent

journal.cli.transcribe.error.writer.speech.loud.windows.is.required.when.loud.windows.is = { $code_speech_loud_windows } is required when { $code_loud_windows } is positive

journal.cli.transcribe.error.writer.stage.path.already.exists = stage path already exists

journal.cli.transcribe.error.writer.stage.path.is.a.symlink = stage path is a symlink

journal.cli.transcribe.error.writer.stage.path.is.not.a.private.directory = stage path is not a private directory

journal.cli.transcribe.error.writer.stale.npz.unlink.refused = stale npz unlink refused

journal.cli.transcribe.error.writer.statement = statement { $index }: { $detail }

journal.cli.transcribe.error.writer.statement.duplicate = statement { $index } duplicates id { $id }

journal.cli.transcribe.error.writer.statement.id.is.not.a.transcript.statement.id = statement ID is not a transcript statement ID

journal.cli.transcribe.error.writer.statement.id.is.outside.i32 = statement ID is outside { $code_i32 }

journal.cli.transcribe.error.writer.statement.id.missing = statement { $index } is missing id

journal.cli.transcribe.error.writer.statement.ids.count.does.not.match.shape = { $code_statement_ids } count does not match shape

journal.cli.transcribe.error.writer.statement.ids.must.be.unique = statement IDs must be unique

journal.cli.transcribe.error.writer.statement.text.ended.before.its.trimmed.length = statement text ended before its trimmed length

journal.cli.transcribe.error.writer.statement.text.is.not.valid.utf8 = statement text is not valid { $code_utf_8 }

journal.cli.transcribe.error.writer.text.must.be.a.nonempty.string = text must be a nonempty string

journal.cli.transcribe.error.writer.turn.audio.duration.s.must.be.non.negative = { $code_audio_duration_s } must be non-negative

journal.cli.transcribe.error.writer.turn.boundary.suspect.must.be.true.if.present = { $code_turn_boundary_suspect } must be true if present

journal.cli.transcribe.error.writer.turn.expected.version.expected.got.version = expected version { $expected }, got { $version }

journal.cli.transcribe.error.writer.turn.field.is.missing = { $field } is missing

journal.cli.transcribe.error.writer.turn.field.must.be.finite = { $field } must be finite

journal.cli.transcribe.error.writer.turn.invalid = invalid { $code_speaker_turn_evidence }: { $detail }

journal.cli.transcribe.error.writer.turn.range.index.end.s.end.s.exceeds.audio = range { $index } { $code_end_s } { $end_s } exceeds { $code_audio_duration_s } { $audio_duration_s }

journal.cli.transcribe.error.writer.turn.range.index.has.unexpected.keys = range { $index } has unexpected keys

journal.cli.transcribe.error.writer.turn.range.index.is.an.exact.duplicate = range { $index } is an exact duplicate

journal.cli.transcribe.error.writer.turn.range.index.is.inverted.start.s.start.s = range { $index } is inverted: { $code_start_s } { $start_s } > { $code_end_s } { $end_s }

journal.cli.transcribe.error.writer.turn.range.index.must.be.an.object = range { $index } must be an object

journal.cli.transcribe.error.writer.turn.range.index.start.s.must.be.non.negative = range { $index } { $code_start_s } must be non-negative

journal.cli.transcribe.error.writer.turn.ranges.must.be.an.array = ranges must be an array

journal.cli.transcribe.error.writer.turn.ranges.must.be.sorted.range.index.is.out = ranges must be sorted: range { $index } is out of order

journal.cli.transcribe.error.writer.turn.speaker.turns.evidence.has.unexpected.keys = speaker turns evidence has unexpected keys

journal.cli.transcribe.error.writer.turn.speaker.turns.evidence.must.be.an.object = speaker turns evidence must be an object

journal.cli.transcribe.error.writer.turn.version.must.be.a.string = version must be a string

journal.cli.transcribe.error.writer.zip.central.extra.at.extra.at.error = central extra at { $extra_at }: { $error }

journal.cli.transcribe.error.writer.zip.central.header.at.central.header.start.error = central header at { $central_header_start }: { $error }

journal.cli.transcribe.error.writer.zip.central.name.at.name.at.error = central name at { $name_at }: { $error }

journal.cli.transcribe.error.writer.zip.central.record.extends.beyond.archive = central record extends beyond archive

journal.cli.transcribe.error.writer.zip.central.signature.at.central.header.start = central signature at { $central_header_start }

journal.cli.transcribe.error.writer.zip.cp437.scalar = { $code_cp437 } scalar

journal.cli.transcribe.error.writer.zip.flagged.name.is.not.utf8 = flagged name is not { $code_utf_8 }

journal.cli.transcribe.error.writer.zip.local.header.at.header.start.error = local header at { $header_start }: { $error }

journal.cli.transcribe.error.writer.zip.local.signature.at.header.start = local signature at { $header_start }

journal.cli.transcribe.error.writer.zip.raw.and.exposed.central.populations.disagree = raw and exposed central populations disagree

journal.cli.transcribe.error.writer.zip.raw.central.member.was.lost.from.the.zip = raw central member was lost from the ZIP index

journal.cli.transcribe.error.writer.zip.repeated.exposed.central.coordinate = repeated exposed central coordinate

journal.cli.transcribe.error.writer.zip.unicode.path.extra.crc.mismatch = unicode path extra crc mismatch

journal.cli.transcribe.error.writer.zip.unicode.path.extra.is.not.utf8 = unicode path extra is not { $code_utf_8 }

journal.cli.transcribe.error.writer.zip.unicode.path.extra.too.small = unicode path extra too small

journal.cli.transcribe.error.writer.zip.unproved = zip member { $index } name unproved: { $detail }

journal.cli.transcribe.error.writer.zip.unreadable = zip archive unreadable: { $detail }

journal.cli.transcribe.error.writer.zip.zip.member.name.collision.name = zip member name collision: { $name }

journal.cli.transcribe.help = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $syntax_continuation_3 }{ $line_break }{ $line_break }Transcribe audio files using pluggable { $stt } and native speaker analysis{ $line_break }{ $line_break }positional arguments:{ $line_break }  { $input }            Path to audio file in journal segment directory, e.g.{ $line_break }                        { $sample }{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }show this help message and exit{ $line_break }{ $indent_2 }{ $all_option }{ $gap_17 }Batch-transcribe all unprocessed audio segments in the{ $line_break }                        journal{ $line_break }{ $indent_2 }{ $redo_option }{ $gap_16 }Reprocess file, overwriting existing outputs{ $line_break }{ $syntax_continuation_16 }{ $line_break }                        { $stt } backend to use (overrides config and resource-{ $line_break }                        aware auto default){ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_9 }Enable verbose output{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_11 }Enable debug logging

journal.cli.transcribe.installation.capability.mismatch = generation capability does not match the current installation

journal.cli.transcribe.installation.coded = { $code }: { $detail }

journal.cli.transcribe.installation.fd.platform = { $fd }: unsupported platform

journal.cli.transcribe.installation.grant.kind = unexpected generation grant kind

journal.cli.transcribe.installation.grant.required = authenticated generation grant is missing or duplicated

journal.cli.transcribe.installation.helper.parent = { $helper }: current executable has no parent directory

journal.cli.transcribe.installation.id.encoding = non-{ $unicode } generation ID

journal.cli.transcribe.installation.lock.kind = generation lock is not an ordinary file

journal.cli.transcribe.installation.metadata.malformed = malformed generation metadata

journal.cli.transcribe.installation.metadata.partial = partial generation metadata

journal.cli.transcribe.installation.metadata.required = generation metadata is missing

journal.cli.transcribe.installation.metadata.unverified = generation metadata has no authenticated launch capability

journal.cli.transcribe.installation.obsolete = obsolete { $windows } generation descriptor marker

journal.cli.transcribe.installation.platform = { $code }: { $platform }/{ $architecture }

journal.cli.transcribe.installation.provenance = generation launch provenance does not match this root

journal.cli.transcribe.installation.stamp = { $code }: { $path }: { $error }

journal.cli.transcribe.installation.token.encoding = non-{ $unicode } generation token

journal.cli.transcribe.selection.conflict = { $all_flag } and { $input } are mutually exclusive

journal.cli.transcribe.selection.missing = provide { $input } or { $all_flag }

journal.cli.transcribe.sound.all.windows.failed = sound tagging failed for all windows: { $detail }

journal.cli.transcribe.sound.engine.absent = ced engine is not installed

journal.cli.transcribe.sound.failed = sound tagging failed: { $error }

journal.cli.transcribe.sound.helper.executable = ced helper binary is not executable: { $path }

journal.cli.transcribe.sound.helper.exit = ced helper exited { $code }: { $stderr }

journal.cli.transcribe.sound.helper.io = ced helper I/O failed: { $detail }

journal.cli.transcribe.sound.helper.large = ced response exceeded 64 MiB

journal.cli.transcribe.sound.helper.missing = ced helper binary is missing: { $path }

journal.cli.transcribe.sound.helper.parent = current executable has no parent directory

journal.cli.transcribe.sound.helper.response = ced helper response was not valid JSON: { $detail }

journal.cli.transcribe.sound.helper.spawn = ced helper could not start: { $detail }

journal.cli.transcribe.sound.helper.timeout = ced helper did not exit within { $timeout }

journal.cli.transcribe.sound.helper.unresolved = ced helper unresolved: { $detail }

journal.cli.transcribe.sound.helper.windows = CED requires the Windows host caller

journal.cli.transcribe.sound.package.platform = unsupported CED platform { $os }/{ $arch }

journal.cli.transcribe.sound.package.windows.runtime = Windows CED package verification requires a Windows runtime

journal.cli.transcribe.sound.prepare = sound tagging could not prepare audio: { $error }

journal.cli.transcribe.sound.probe.helper.exit = ced probe helper exited { $code }: { $stderr }

journal.cli.transcribe.sound.probe.helper.unexpected = ced probe helper returned an unexpected response: { $response }

journal.cli.transcribe.sound.probe.unexpected = ced probe returned an unexpected response: { $response }

journal.cli.transcribe.sound.schema = sound tagging returned an unexpected response schema

journal.cli.transcribe.sound.unsupported = sound tagger disabled: ced assets unsupported on { $os }/{ $arch }

journal.cli.transcribe.sound.window.count = sound tagging returned an unexpected window count

journal.cli.transcribe.sound.window.ok = sound tagging returned a window missing an { $field } field

journal.cli.transcribe.sound.windows.launch = CED helper launch failed: { $error }

journal.cli.transcribe.sound.windows.library = CED request library does not match the signed package

journal.cli.transcribe.sound.windows.package.bin = running executable is not in the package bin directory

journal.cli.transcribe.sound.windows.package.member = signed CED app payload refuses { $member }: { $error }

journal.cli.transcribe.sound.windows.package.parent = running executable has no containing directory

journal.cli.transcribe.sound.windows.package.root = package bin directory has no package root

journal.cli.transcribe.sound.windows.probe = CED helper returned an invalid probe response

journal.cli.transcribe.sound.windows.root = { $variable } is not set

journal.cli.transcribe.sound.windows.signed = Windows CED requires the signed package helper and its declared command

journal.cli.transcribe.stage.confidential.cap = audio is { $seconds }s and would send { $bytes } bytes; confidential STT accepts at most { $limit } bytes

journal.cli.transcribe.stage.confidential.verification.clear = confidential transcription verification was not cleared for { $path }: { $error }

journal.cli.transcribe.stage.confidential.verification.record = confidential transcription verification was not recorded for { $path }: { $error }

journal.cli.transcribe.stage.embedded.missing.draft = recognize walk has no draft spool

journal.cli.transcribe.stage.embedded.range.missing = merged turn range is missing

journal.cli.transcribe.stage.embedded.scratch = could not create embedded scratch: { $error }

journal.cli.transcribe.stage.output.parent = transcript path has no parent

journal.cli.transcribe.stage.owner.deleted.unrecorded = voice-only original deleted but deletion event was not recorded

journal.cli.transcribe.stage.owner.helper.outdated = the speaker-analysis helper predates overlap ranges or speaker turns

journal.cli.transcribe.stage.owner.not.deleted = voice-only original not deleted now: { $outcome }

journal.cli.transcribe.stage.owner.save.or.disable = save a voice in Speakers, or turn off { $setting }

journal.cli.transcribe.stage.owner.save.to.finish = save a voice in Speakers to finish this filtered recording

journal.cli.transcribe.usage.error = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $syntax_continuation_3 }{ $line_break }{ $command }: error: { $message }

journal.cli.depict.admission = journal process admission failed: { $detail }

journal.cli.depict.admission.binding.mismatch = launch installation binding mismatch

journal.cli.depict.admission.birth.invalid = { $linux } process birth has a malformed { $boot }

journal.cli.depict.admission.birth.position = { $linux } process birth has a malformed { $boot } at line { $line } column { $column }

journal.cli.depict.admission.byte.count = launch pipe returned an invalid byte count

journal.cli.depict.admission.cleanup.pending = previous launch { $io } cleanup remains incomplete

journal.cli.depict.admission.commit.mismatch = launch admission commit mismatch

journal.cli.depict.admission.deadline.elapsed = launch deadline elapsed

journal.cli.depict.admission.deadline.invalid = expired or invalid { $windows } launch deadline

journal.cli.depict.admission.descriptor.encoding = non-{ $unicode } { $windows } launch descriptor

journal.cli.depict.admission.descriptor.size = oversized { $windows } launch descriptor

journal.cli.depict.admission.endpoint = invalid launch endpoint

journal.cli.depict.admission.entry.hosted = installed task at hosted entry

journal.cli.depict.admission.entry.mismatch = launch variant or installed action mismatch

journal.cli.depict.admission.frame.size = invalid launch frame size

journal.cli.depict.admission.grant.access = launch handle access or type does not match its grant

journal.cli.depict.admission.grant.disk = launch grant is not a disk file

journal.cli.depict.admission.grant.duplicate = duplicate launch handle or grant kind

journal.cli.depict.admission.grant.ordinary = launch grant is not an ordinary disk file

journal.cli.depict.admission.guard.duplicate = installation guard: duplicate identity guard { $field }

journal.cli.depict.admission.guard.encoding = non-{ $unicode } installation guard

journal.cli.depict.admission.guard.invalid = installation guard: invalid identity guard { $field }

journal.cli.depict.admission.guard.mismatch = launch guard tuple differs from inherited guard

journal.cli.depict.admission.guard.missing = installation guard: identity guard fields are incomplete

journal.cli.depict.admission.handle.invalid = invalid transferred handle

journal.cli.depict.admission.identity.io.local.folder = resolve { $local_data } known folder: { $error }

journal.cli.depict.admission.identity.io.marker.clone = clone adoption marker: { $error }

journal.cli.depict.admission.identity.io.marker.read = read adoption marker: { $error }

journal.cli.depict.admission.identity.io.marker.stat = stat adoption marker: { $error }

journal.cli.depict.admission.identity.io.namespace.read = read namespace directory: { $error }

journal.cli.depict.admission.identity.io.owner.open = open owner directory: { $error }

journal.cli.depict.admission.identity.io.record.read = read identity record: { $error }

journal.cli.depict.admission.identity.io.record.stat = stat identity record: { $error }

journal.cli.depict.admission.identity.io.root = canonicalize root: { $error }

journal.cli.depict.admission.identity.io.storage.attrs = query storage attributes: { $error }

journal.cli.depict.admission.identity.io.storage.file = open storage file: { $error }

journal.cli.depict.admission.identity.io.storage.identity = query storage identity: { $error }

journal.cli.depict.admission.identity.io.storage.lock = lock identity storage: { $error }

journal.cli.depict.admission.identity.io.storage.open = open storage directory: { $error }

journal.cli.depict.admission.identity.local.encoding = { $local_data } known folder is not valid { $utf }

journal.cli.depict.admission.identity.marker.content = adoption marker has invalid content

journal.cli.depict.admission.identity.marker.missing = committed record has no adoption marker

journal.cli.depict.admission.identity.marker.orphan = orphan adoption marker

journal.cli.depict.admission.identity.marker.size = adoption marker has invalid size

journal.cli.depict.admission.identity.namespace.child = unknown namespace child

journal.cli.depict.admission.identity.owner.poisoned = owner coordinator is poisoned

journal.cli.depict.admission.identity.path.device = { $windows } verbatim and device paths are not accepted

journal.cli.depict.admission.identity.path.encoding = { $windows } path is not valid { $utf }

journal.cli.depict.admission.identity.path.escape = { $windows } path escapes its absolute root

journal.cli.depict.admission.identity.path.nul = { $windows } path contains an interior { $nul }

journal.cli.depict.admission.identity.path.relative = { $windows } path must be fully qualified

journal.cli.depict.admission.identity.path.server = { $windows } { $unc } path has no server

journal.cli.depict.admission.identity.path.share = { $windows } { $unc } path has no share

journal.cli.depict.admission.identity.path.size = path exceeds { $limit } bytes

journal.cli.depict.admission.identity.prepared = installation identity record is prepared

journal.cli.depict.admission.identity.record.missing = namespace record is missing

journal.cli.depict.admission.identity.record.size = identity record exceeds bounds

journal.cli.depict.admission.identity.root.mismatch = record does not bind the requested root

journal.cli.depict.admission.identity.storage.directory = storage path is not a directory

journal.cli.depict.admission.identity.storage.file = storage path is not a regular file

journal.cli.depict.admission.identity.storage.reparse = storage path is a reparse point

journal.cli.depict.admission.identity.tombstoned = installation identity record is tombstoned

journal.cli.depict.admission.nonce = invalid launch endpoint nonce

journal.cli.depict.admission.obsolete = obsolete { $windows } launch marker

journal.cli.depict.admission.offer.mismatch = launch offer provenance mismatch

journal.cli.depict.admission.parent.mismatch = launch server is not the exact direct parent

journal.cli.depict.admission.parent.provenance = invalid parent launch provenance

journal.cli.depict.admission.peer.unverified = launch pipe peer is not verifiably live

journal.cli.depict.admission.process.creation = current { $windows } process creation { $filetime } is unavailable

journal.cli.depict.admission.process.signaled = current { $windows } process is signaled

journal.cli.depict.admission.provenance = invalid launch provenance

journal.cli.depict.admission.record.ascii = identity record is not { $ascii }

journal.cli.depict.admission.record.checksum = identity record checksum mismatch

journal.cli.depict.admission.record.field = invalid identity record { $field }

journal.cli.depict.admission.record.lines = identity record is not exactly eight { $linefeed } lines

journal.cli.depict.admission.record.size = identity record exceeds { $size }

journal.cli.depict.admission.root.missing = launch executable has no installation root

journal.cli.depict.admission.server.mismatch = launch pipe server identity mismatch

journal.cli.depict.admission.snapshot.absent = current process is absent from the parent snapshot

journal.cli.depict.admission.task.provenance = invalid installed task provenance

journal.cli.depict.admission.wait.complete = launch { $io } wait failed: { $error }; { $io } cancellation completed

journal.cli.depict.admission.wait.deadline.complete = launch deadline elapsed; { $io } cancellation completed

journal.cli.depict.admission.wait.deadline.incomplete = launch deadline elapsed; { $io } cleanup incomplete and retained

journal.cli.depict.admission.wait.incomplete = launch { $io } wait failed: { $error }; { $io } cleanup incomplete and retained

journal.cli.depict.detection.unavailable = Object detection is unavailable. Run `{ $command }` to check or repair the { $engine } assets.

journal.cli.depict.detection.unavailable.windows = object detection is unavailable. repair or reinstall your journal.

journal.cli.depict.help = usage: { $syntax }

journal.cli.depict.metadata.invalid = native depict: invalid { $variable } { $format }

journal.cli.describe.cancelled = describe cancelled

journal.cli.describe.decode.failed = failed to decode video: { $path }

journal.cli.describe.deferred = describe deferred

journal.cli.describe.deferred.reason = describe deferred: { $reason }

journal.cli.describe.faces.skipped = face enrichment skipped: { $kind }

journal.cli.describe.file.encoding = video path must have a { $encoding } filename

journal.cli.describe.output.encode = failed to encode output: { $error }

journal.cli.describe.output.write = failed to write output: { $error }

journal.cli.describe.path.config = could not read journal path: { $reason }

journal.cli.describe.path.home = could not resolve home directory: { $reason }

journal.cli.describe.pipeline.flat.bound = bound directory name exists and is not a directory

journal.cli.describe.pipeline.flat.create.flat.directory.child = create flat-directory child failed for { $path }: { $detail }

journal.cli.describe.pipeline.flat.normal = name must be one normal component

journal.cli.describe.pipeline.flat.read.flat.directory.identity = read flat-directory identity failed for { $path }: { $detail }

journal.cli.describe.pipeline.flat.relative = path must be a nonempty sequence of normal portable components

journal.cli.describe.pipeline.flat.revalidate.flat.directory = revalidate flat directory failed for { $path }: { $detail }

journal.cli.describe.pipeline.flat.stat.flat.directory.descendant = stat flat-directory descendant failed for { $path }: { $detail }

journal.cli.describe.pipeline.flat.stat.opened.flat.directory.descendant = stat opened flat-directory descendant failed for { $path }: { $detail }

journal.cli.describe.pipeline.flat.utf8 = the name is not valid { $encoding } text

journal.cli.describe.pipeline.frames.failed = all describe frame requests failed

journal.cli.describe.pipeline.install.observe.installed.destination.after.move = { $path }: observe installed destination after move: { $detail }

journal.cli.describe.pipeline.install.reconcile.install.after.failed.move = { $path }: reconcile install after failed move: { $detail }

journal.cli.describe.pipeline.install.revalidate.install.paths.after.move = { $path }: revalidate install paths after move: { $detail }

journal.cli.describe.pipeline.jobs = { $flag } must be positive

journal.cli.describe.pipeline.merge.frame = missing merge frame

journal.cli.describe.pipeline.metadata.object = { $segment_meta } must be an object

journal.cli.describe.pipeline.ocr.coverage = screen { $ocr } coverage is incomplete; private frame diagnostics record the gap

journal.cli.describe.pipeline.ocr.publication = screen { $ocr } publication failed: { $kind }

journal.cli.describe.pipeline.ocr.retire = screen { $ocr } stale aggregate could not be retired: { $kind }

journal.cli.describe.pipeline.parent = video has no parent

journal.cli.describe.pipeline.progress = describe frame window made no progress

journal.cli.describe.pipeline.publish.anchor.is.not.a.mount.manager.volume.guid.path = anchor is not a { $manager } volume { $guid } path

journal.cli.describe.pipeline.publish.changed = prepared publication changed

journal.cli.describe.pipeline.publish.component = publication leaf is not a plain component

journal.cli.describe.pipeline.publish.current.directory.is.not.valid.utf = current directory is not valid { $encoding }

journal.cli.describe.pipeline.publish.destination = publication destination is not a plain file

journal.cli.describe.pipeline.publish.destination.already.identifies.the.source.file = destination already identifies the source file

journal.cli.describe.pipeline.publish.destination.changed.during.publication = destination changed during publication

journal.cli.describe.pipeline.publish.destination.has.no.normal.name = destination has no normal name

journal.cli.describe.pipeline.publish.destination.is.a.reparse.point = destination is a reparse point

journal.cli.describe.pipeline.publish.destination.is.not.a.regular.file = destination is not a regular file

journal.cli.describe.pipeline.publish.identity = prepared publication identity changed

journal.cli.describe.pipeline.publish.identity.value.out.of.range = identity value out of range

journal.cli.describe.pipeline.publish.install.publication.exhausted = install publication exhausted

journal.cli.describe.pipeline.publish.leaf = publication leaf missing

journal.cli.describe.pipeline.publish.mode.exceeds.o = mode exceeds { $octal }

journal.cli.describe.pipeline.publish.move.spelling.contains.an.interior.nul = move spelling contains an interior { $nul }

journal.cli.describe.pipeline.publish.parent = publication parent missing

journal.cli.describe.pipeline.publish.path = { $path }: { $detail }

journal.cli.describe.pipeline.publish.publication.path.is.not.valid.utf = publication path is not valid { $encoding }

journal.cli.describe.pipeline.publish.published.destination.identity.does.not.match.the.live.source = published destination identity does not match the live source

journal.cli.describe.pipeline.publish.published.destination.is.not.a.regular.file = published destination is not a regular file

journal.cli.describe.pipeline.publish.published.destination.name.is.absent = published destination name is absent

journal.cli.describe.pipeline.publish.source.and.destination.are.on.different.volumes = source and destination are on different volumes

journal.cli.describe.pipeline.publish.source.and.destination.name.the.same.file = source and destination name the same file

journal.cli.describe.pipeline.publish.source.has.no.normal.name = source has no normal name

journal.cli.describe.pipeline.publish.source.identity.changed.before.publication = source identity changed before publication

journal.cli.describe.pipeline.publish.source.identity.is.negative = source identity is negative

journal.cli.describe.pipeline.publish.source.name.changed.before.publication = source name changed before publication

journal.cli.describe.pipeline.publish.source.name.is.absent.before.publication = source name is absent before publication

journal.cli.describe.pipeline.publish.sync = describe publication parent sync failed: { $error }

journal.cli.describe.pipeline.publish.uncertain = { $path }: { $operation }: { $detail }

journal.cli.describe.pipeline.reappear = describe frame { $frame } did not reappear with the same timestamp

journal.cli.describe.pipeline.record.parent.changed = screen text parent changed

journal.cli.describe.pipeline.record.parent.plain = screen text parent is not a plain directory

journal.cli.describe.pipeline.reopen = describe could not reopen the video to materialize frame payloads

journal.cli.describe.pipeline.resize = failed to resize categorization frame

journal.cli.describe.pipeline.response.id = session response omitted id

journal.cli.describe.pipeline.response.unmatched = uncorrelated response

journal.cli.describe.pipeline.retry.binding = { $ocr } retry frame binding changed

journal.cli.describe.pipeline.retry.evidence.missing = missing retry evidence

journal.cli.describe.pipeline.retry.handler = native failure handler changed

journal.cli.describe.pipeline.retry.id = native retry identity is not { $encoding }

journal.cli.describe.pipeline.retry.incomplete = { $ocr } retry did not reach the last qualified frame

journal.cli.describe.pipeline.retry.input.changed = retry input changed while reading

journal.cli.describe.pipeline.retry.input.regular = retry input is not a regular file

journal.cli.describe.pipeline.retry.io.allocate = could not allocate stage

journal.cli.describe.pipeline.retry.io.dest.namespace = destination namespace changed before publication

journal.cli.describe.pipeline.retry.io.file.kind = entry is not a regular file

journal.cli.describe.pipeline.retry.io.file.reparse = entry is a reparse point

journal.cli.describe.pipeline.retry.io.fsync = parent-directory fsync degraded for { $path }: { $error }

journal.cli.describe.pipeline.retry.io.parent.identity = bound parent identity changed before publication

journal.cli.describe.pipeline.retry.io.parent.kind = parent is not a real directory

journal.cli.describe.pipeline.retry.io.parent.rename = parent pathname changed before publication

journal.cli.describe.pipeline.retry.io.parent.reparse = parent is a reparse point

journal.cli.describe.pipeline.retry.io.prepare = stage identity changed while preparing

journal.cli.describe.pipeline.retry.io.stage.identity = stage identity changed before publication

journal.cli.describe.pipeline.retry.io.stage.namespace = stage namespace changed before publication

journal.cli.describe.pipeline.retry.io.tmp = could not allocate unique temporary file

journal.cli.describe.pipeline.retry.journal = native retry journal does not match

journal.cli.describe.pipeline.retry.open = could not reopen video for { $ocr } retry

journal.cli.describe.pipeline.retry.permission.changed = native retry permission changed

journal.cli.describe.pipeline.retry.permission.missing = native retry permission is missing

journal.cli.describe.pipeline.root.canonicalize.journal.root = canonicalize journal root: { $detail }

journal.cli.describe.pipeline.root.cloud.files.sync.root.status.could.not.be.verified = { $cloud_files } sync-root status could not be verified

journal.cli.describe.pipeline.root.filesystem.is.not.ntfs.or.refs = filesystem is not { $ntfs } or { $refs }

journal.cli.describe.pipeline.root.filesystem.type.could.not.be.verified = filesystem type could not be verified

journal.cli.describe.pipeline.root.journal.root.contains.a.reparse.point = journal root contains a reparse point

journal.cli.describe.pipeline.root.journal.root.contains.an.interior.nul = journal root contains an interior { $nul }

journal.cli.describe.pipeline.root.journal.root.has.an.invalid.portable.name = journal root has an invalid portable name

journal.cli.describe.pipeline.root.journal.root.is.a.cloud.files.sync.root = journal root is a { $cloud_files } sync root

journal.cli.describe.pipeline.root.journal.root.is.a.reparse.point = journal root is a reparse point

journal.cli.describe.pipeline.root.journal.root.is.on.a.non.local.volume = journal root is on a non-local volume

journal.cli.describe.pipeline.root.journal.root.is.on.a.ram.disk = journal root is on a { $ram } disk

journal.cli.describe.pipeline.root.journal.root.is.on.removable.or.optical.media = journal root is on removable or optical media

journal.cli.describe.pipeline.root.journal.root.must.be.lexically.canonical = journal root must be lexically canonical

journal.cli.describe.pipeline.root.journal.root.must.use.a.fully.qualified.drive.path = journal root must use a fully qualified drive path

journal.cli.describe.pipeline.root.journal.root.volume.type.is.unknown = journal root volume type is unknown

journal.cli.describe.pipeline.root.open.filesystem.root = open filesystem root: { $detail }

journal.cli.describe.pipeline.root.open.journal.directory = open journal directory: { $detail }

journal.cli.describe.pipeline.root.open.journal.root = open journal root: { $detail }

journal.cli.describe.pipeline.root.open.journal.root.ancestor = open journal root ancestor: { $detail }

journal.cli.describe.pipeline.root.open.journal.root.namespace.watch = open journal root namespace watch: { $detail }

journal.cli.describe.pipeline.root.open.verified.drive.root = open verified drive root: { $detail }

journal.cli.describe.pipeline.root.open.verified.journal.root = open verified journal root: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.ancestor.attributes = query journal root ancestor attributes: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.attributes = query journal root attributes: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.drive.type = query journal root drive type: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.identity = query journal root identity: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.namespace.watch.attributes = query journal root namespace-watch attributes: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.namespace.watch.identity = query journal root namespace-watch identity: { $detail }

journal.cli.describe.pipeline.root.query.retained.journal.root.attributes = query retained journal root attributes: { $detail }

journal.cli.describe.pipeline.root.query.retained.journal.root.identity = query retained journal root identity: { $detail }

journal.cli.describe.pipeline.root.query.verified.drive.root.attributes = query verified drive root attributes: { $detail }

journal.cli.describe.pipeline.root.query.verified.drive.root.identity = query verified drive root identity: { $detail }

journal.cli.describe.pipeline.root.query.verified.journal.root.attributes = query verified journal root attributes: { $detail }

journal.cli.describe.pipeline.root.query.verified.journal.root.identity = query verified journal root identity: { $detail }

journal.cli.describe.pipeline.root.read.source.file.identity = read source file identity: { $detail }

journal.cli.describe.pipeline.root.restat.acquired.journal.root = restat acquired journal root: { $detail }

journal.cli.describe.pipeline.root.stat.acquired.journal.root = stat acquired journal root: { $detail }

journal.cli.describe.pipeline.root.stat.canonical.root.component = stat canonical root component: { $detail }

journal.cli.describe.pipeline.root.stat.opened.canonical.root.component = stat opened canonical root component: { $detail }

journal.cli.describe.pipeline.root.stat.opened.filesystem.root = stat opened filesystem root: { $detail }

journal.cli.describe.pipeline.row.frame = describe row omitted { $frame_key }

journal.cli.describe.pipeline.source.bytes = { $ocr } source bytes changed

journal.cli.describe.pipeline.source.changed = { $ocr } source changed before publication

journal.cli.describe.pipeline.source.identity = { $ocr } source identity changed

journal.cli.describe.pipeline.source.open = { $ocr } source changed while opening

journal.cli.describe.pipeline.source.plain = { $ocr } source is not a plain file

journal.cli.describe.pipeline.temp = could not allocate describe temp file

journal.cli.describe.pipeline.timestamp = describe frame { $frame } omitted timestamp

journal.cli.describe.pipeline.windows.cleanup = { $detail }; could not remove stage { $stage }: { $error }

journal.cli.describe.pipeline.windows.name.alternate.data.stream = the name contains a colon

journal.cli.describe.pipeline.windows.name.control = the name contains a control character

journal.cli.describe.pipeline.windows.name.dot.component = '.' and '..' are not valid names

journal.cli.describe.pipeline.windows.name.empty = the name is empty

journal.cli.describe.pipeline.windows.name.forbidden.character = the name contains a character { $windows } does not allow

journal.cli.describe.pipeline.windows.name.reserved.device = the name is reserved by { $windows }

journal.cli.describe.pipeline.windows.name.root.or.prefix = the name must be a single relative component

journal.cli.describe.pipeline.windows.name.separator = the name contains a path separator

journal.cli.describe.pipeline.windows.name.too.long = the name is longer than { $length_limit } { $encoding } bytes

journal.cli.describe.pipeline.windows.name.trailing.dot.or.space = the name ends in a dot or space

journal.cli.describe.pipeline.windows.path.already.exists = publication path ancestor '{ $component }' appeared during create-only

journal.cli.describe.pipeline.windows.path.classify.install.source = classify install source: { $detail }

journal.cli.describe.pipeline.windows.path.component = publication path component is not portable: { $detail }

journal.cli.describe.pipeline.windows.path.create.publication.path.ancestor = create publication-path ancestor: { $detail }

journal.cli.describe.pipeline.windows.path.dot.component = publication path contains '.' or '..'

journal.cli.describe.pipeline.windows.path.drive.relative = publication path is drive-relative

journal.cli.describe.pipeline.windows.path.empty = publication path is empty

journal.cli.describe.pipeline.windows.path.identity.changed = publication path identity changed at retained level { $level }

journal.cli.describe.pipeline.windows.path.identity.mismatch = publication path ancestor '{ $component }' identity changed between create and reopen

journal.cli.describe.pipeline.windows.path.interior.nul = publication path contains an interior { $nul }

journal.cli.describe.pipeline.windows.path.missing = publication path component '{ $component }' does not exist

journal.cli.describe.pipeline.windows.path.move.spelling.diverged = publication path move spelling no longer names the terminal parent

journal.cli.describe.pipeline.windows.path.not.directory = publication path ancestor '{ $component }' is not a directory

journal.cli.describe.pipeline.windows.path.not.regular.file = publication path leaf '{ $component }' is not a regular file

journal.cli.describe.pipeline.windows.path.open.install.source = open install source: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.ancestor = open publication-path ancestor: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.anchor = open publication-path anchor: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.move.spelling = open publication-path move spelling: { $detail }

journal.cli.describe.pipeline.windows.path.path.too.long = publication path exceeds the extended-length bound

journal.cli.describe.pipeline.windows.path.query.install.path.attributes = query install path attributes: { $detail }

journal.cli.describe.pipeline.windows.path.query.install.source.identity = query install source identity: { $detail }

journal.cli.describe.pipeline.windows.path.query.publication.path.attributes = query publication-path attributes: { $detail }

journal.cli.describe.pipeline.windows.path.query.publication.path.identity = query publication-path identity: { $detail }

journal.cli.describe.pipeline.windows.path.reopen.publication.path.ancestor = reopen publication-path ancestor: { $detail }

journal.cli.describe.pipeline.windows.path.reparse.point = publication path ancestor '{ $component }' is a reparse point

journal.cli.describe.pipeline.windows.path.resolve.publication.path.current.directory = resolve publication-path current directory: { $detail }

journal.cli.describe.pipeline.windows.path.revalidate.install.source.after.classification = revalidate install source after classification: { $detail }

journal.cli.describe.pipeline.windows.path.root.only = publication path is a drive root with no destination

journal.cli.describe.pipeline.windows.path.rooted.without.drive = publication path is rooted without a drive

journal.cli.describe.pipeline.windows.path.unc.or.device.prefix = publication path has a { $unc } or device prefix

journal.cli.describe.pipeline.windows.path.unstable = publication path has no { $manager } volume { $guid }: { $detail }

journal.cli.describe.pipeline.withdraw = screen { $ocr } withdrawal could not be prepared: { $detail }

journal.cli.describe.settings.finite = { $setting } must be a finite number

journal.cli.describe.settings.importance = { $setting } must be { $ignore }, { $low }, { $normal }, or { $high }

journal.cli.describe.settings.object = { $setting } must be an object

journal.cli.describe.settings.string = { $setting } must be a string

journal.cli.describe.settings.strings = { $setting } must be an array of strings

journal.cli.describe.settings.uint = { $setting } must be an unsigned 32-bit integer

journal.cli.describe.stored.error.empty = Empty markdown response for { $category }

journal.cli.describe.stored.error.incomplete = Extraction never completed

journal.cli.describe.stored.error.invalid = Invalid { $format } response

journal.cli.describe.stored.error.invalid.category = Invalid { $format } response for { $category }: { $error }

journal.cli.describe.stored.error.resize = failed to resize extraction frame

journal.cli.describe.stored.error.schema = Schema validation failed for { $category }

journal.cli.describe.stored.error.schema.detail = Schema validation failed for { $category }: { $errors }

journal.cli.describe.stored.error.truncated = Truncated markdown response for { $category }

journal.cli.describe.stored.note.bounded = [{ $mark }: extraction output bounded before journaling - degenerate length sanitized/truncated]

journal.cli.describe.stored.note.ocr = text recognized from your screens. it may contain recognition errors. this says what was visible, not what you read, typed or did. masking passwords and other secrets is best effort; your redaction rules do not apply.

journal.cli.describe.stored.ocr.heading = { $clock } · { $category }

journal.cli.sol.help.apps = Apps ({ $invocation }):

journal.cli.sol.help.computer = This computer

journal.cli.sol.help.journal = Your journal

journal.cli.sol.help.title = { $solstone_name } - your journal, from the command line

journal.cli.sol.help.tools = Tools

journal.cli.sol.launcher.cycle.mac = { $solstone_name }: native launcher symlink cycle detected. Reinstall the journal app.

journal.cli.sol.launcher.cycle.unix = { $solstone_name }: native launcher symlink cycle detected. Reinstall { $solstone_name } and { $sibling }.

journal.cli.sol.launcher.dangling.mac = { $solstone_name }: native launcher symlink is dangling: { $path }. Reinstall the journal app.

journal.cli.sol.launcher.dangling.unix = { $solstone_name }: native launcher symlink is dangling: { $path }. Reinstall { $solstone_name } and { $sibling }.

journal.cli.sol.launcher.executable.mac = { $solstone_name }: native { $sibling } sibling is not executable: { $path }. Reinstall the journal app.

journal.cli.sol.launcher.executable.unix = { $solstone_name }: native { $sibling } sibling is not executable: { $path }. Reinstall { $solstone_name } and { $sibling }.

journal.cli.sol.launcher.missing.mac = { $solstone_name }: native { $sibling } sibling is missing: { $path }. Reinstall the journal app.

journal.cli.sol.launcher.missing.unix = { $solstone_name }: native { $sibling } sibling is missing: { $path }. Reinstall { $solstone_name } and { $sibling }.

journal.cli.sol.moved = '{ $command }' moved to '{ $journal_command } { $command }'; run that instead.{ $line_break }(commands that run the journal on this computer are under '{ $journal_command }'; see '{ $journal_help }'.)

journal.cli.sol.resident.platform = shutdown signals are unavailable on this platform

journal.cli.sol.resident.signal = native { $solstone_name } resident signal setup failed: { $error }

journal.cli.sol.resident.startup = native { $solstone_name } resident startup flush failed: { $error }

journal.cli.sol.root.inspect = native { $solstone_name } project root resolution failed: could not inspect current executable: { $error }

journal.cli.sol.root.package = native { $solstone_name } project root resolution failed: could not locate source checkout or installed { $solstone_name } package from { $path }

journal.cli.sol.skills.agent.invalid = { $skill_command }: error: unsupported { $agent_flag } { $agent }

journal.cli.sol.skills.agent.required = { $skill_command }: error: { $agent_flag } requires a value

journal.cli.sol.skills.argument.invalid = { $skill_command }: error: unrecognized argument { $argument }

journal.cli.sol.skills.bundle.linked = bundled skill contains linked or special entries

journal.cli.sol.skills.cleanup.failed = refresh user skills: installed { $path }; old skill cleanup at { $backup } failed: { $error }

journal.cli.sol.skills.config.decode = config journal decode error: { $kind }

journal.cli.sol.skills.config.not.directory = { $path } is not a directory

journal.cli.sol.skills.copy.special = special files are not permitted in user skill trees

journal.cli.sol.skills.copy.symlink = symlinks are not permitted in user skill trees

journal.cli.sol.skills.cwd.failed = { $solstone }: native skills cwd is unavailable: { $error }. Run this command from an accessible directory.

journal.cli.sol.skills.exe.current = cannot determine current exe: { $error }

journal.cli.sol.skills.exe.parent = current exe has no parent directory

journal.cli.sol.skills.file.utf = { $skill_file } is not valid { $encoding }: { $error }

journal.cli.sol.skills.home.absolute = home must be an absolute path

journal.cli.sol.skills.home.missing = { $solstone }: native skills home is unavailable. Set { $home } to your home directory.

journal.cli.sol.skills.identity.census = census read failed: { $detail }

journal.cli.sol.skills.identity.home.must.be.absolute = home must be absolute

journal.cli.sol.skills.identity.installation.identity.is.unsupported.for.android = installation identity is unsupported for Android

journal.cli.sol.skills.identity.io.clone.namespace.directory = clone namespace directory: { $error }

journal.cli.sol.skills.identity.io.clone.namespace.registry = clone namespace registry: { $error }

journal.cli.sol.skills.identity.io.open.home.directory = open home directory: { $error }

journal.cli.sol.skills.identity.io.open.namespace.directory = open namespace directory: { $error }

journal.cli.sol.skills.identity.io.open.namespace.registry = open namespace registry: { $error }

journal.cli.sol.skills.identity.io.read.namespace.registry = read namespace registry: { $error }

journal.cli.sol.skills.identity.io.stat.storage.directory = stat storage directory: { $error }

journal.cli.sol.skills.identity.io.stat.storage.file = stat storage file: { $error }

journal.cli.sol.skills.identity.namespace.entry.is.not.lowercase.hex = namespace entry is not lowercase hex

journal.cli.sol.skills.identity.namespace.registry.entries.limit = namespace registry exceeds 1024 entries

journal.cli.sol.skills.identity.namespace.registry.size.limit = namespace registry exceeds 16 MiB

journal.cli.sol.skills.identity.path.escapes.its.absolute.root = path escapes its absolute root

journal.cli.sol.skills.identity.path.must.be.absolute.and.nul.free = path must be absolute and NUL-free

journal.cli.sol.skills.identity.root = cannot derive root token from { $path }: { $detail }

journal.cli.sol.skills.identity.storage.directory.does.not.have.private.mode = storage directory does not have mode { $mode }

journal.cli.sol.skills.identity.storage.directory.is.not.owned.by.the.current.user = storage directory is not owned by the current user

journal.cli.sol.skills.identity.storage.file.does.not.have.private.mode = storage file does not have mode { $mode }

journal.cli.sol.skills.identity.storage.file.is.not.owned.by.the.current.user = storage file is not owned by the current user

journal.cli.sol.skills.install.done = installed { $agent } { $skill } -> { $path }

journal.cli.sol.skills.journal.absolute = selected journal must be an absolute path

journal.cli.sol.skills.journal.resolved = resolved journal path is not absolute

journal.cli.sol.skills.operation.error = error: { $operation } { $path }: { $detail }

journal.cli.sol.skills.owner.base = cannot form owner base at { $path }: { $detail }

journal.cli.sol.skills.path.absolute = path must be absolute

journal.cli.sol.skills.path.ancestor.absent = no existing ancestor found for { $path }

journal.cli.sol.skills.path.canonical = canonicalizing ancestor { $path } failed: { $error }

journal.cli.sol.skills.path.inspect = inspecting ancestor { $path } failed: { $error }

journal.cli.sol.skills.path.overlap = path { $path } overlaps protected journal

journal.cli.sol.skills.path.resolved.overlap = resolved ancestor path { $path } overlaps protected journal

journal.cli.sol.skills.payload.failed = { $solstone }: native skills payload root is unavailable: { $detail }. Reinstall { $solstone } and { $core }.

journal.cli.sol.skills.payload.project = error: expected project skill at { $path }

journal.cli.sol.skills.payload.umbrella = error: expected bundled umbrella skill at { $skill_file } ({ $path })

journal.cli.sol.skills.project.agent = error: { $agent_flag } { $agent } is not supported with { $project_flag }; use { $all_option } or { $claude_option }

journal.cli.sol.skills.project.missing = expected project skill at { $path }

journal.cli.sol.skills.publish.rollback = publishing skill failed: { $error }; restoring failed: { $restore }; backup path: { $path }

journal.cli.sol.skills.registry.unknown = installation registry is unknown

journal.cli.sol.skills.remove.directory = refusing to remove non-directory

journal.cli.sol.skills.remove.done = removed { $agent } { $skill } -> { $path }

journal.cli.sol.skills.remove.retired = removed { $agent } { $skill } (retired product skill) -> { $path }

journal.cli.sol.skills.remove.stale = removed { $agent } { $skill } (stale) -> { $path }

journal.cli.sol.skills.remove.symlink = refusing to remove non-symlink

journal.cli.sol.skills.replace.done = replaced { $agent } { $skill } -> { $path }

journal.cli.sol.skills.skip.absent = skipped { $agent } { $skill } (nothing to remove)

journal.cli.sol.skills.skip.all = no AI coding agent config directories found — skipping skill registration

journal.cli.sol.skills.skip.config = skipped { $agent } (config dir absent at { $path })

journal.cli.sol.skills.status.absent = { $agent } { $skill } not installed

journal.cli.sol.skills.status.header = agent      skill                state

journal.cli.sol.skills.status.installed = { $agent } { $skill } installed

journal.cli.sol.skills.subcommand.invalid = { $skill_command }: error: unknown subcommand { $command_name }

journal.cli.sol.skills.subcommand.missing = { $skill_command }: error: missing subcommand

journal.cli.sol.skills.symlink.unsupported = symlink creation unsupported

journal.cli.sol.skills.target.ineligible = target is ineligible for install

journal.cli.sol.skills.target.parent = target has no parent directory

journal.cli.sol.skills.target.preserved = target path overlaps protected journal or is preserved

journal.cli.sol.skills.tree.directory = skill tree requires an ordinary directory

journal.cli.sol.skills.tree.linked = linked entries are not permitted in user skill trees

journal.cli.sol.skills.tree.reference = bundled skill reference requires a directory with a regular { $skill_file }

journal.cli.sol.skills.tree.special = special entries are not permitted in user skill trees

journal.cli.sol.skills.usage = Usage: { $syntax }

journal.cli.sol.skills.warning.existing = warning { $agent } { $skill } -> { $path } (existing content at target preserved)

journal.cli.sol.skills.warning.retired = warning { $agent } { $skill } -> { $path } (does not match the retired { $retired_skill } skill; existing content at { $path } preserved)

journal.cli.sol.skills.warning.stale = warning { $agent } { $skill } -> { $path } (existing content at stale target preserved)

journal.cli.sol.skills.warnings = Warnings:

journal.cli.sol.unsupported = Unsupported native { $solstone_name } command.

journal.cli.sol.update.argument.automatic.conflict = { $automatic_flag } and { $unattended_flag } can only be combined with { $json_flag }

journal.cli.sol.update.argument.automatic.missing = { $automatic_flag } requires { $automatic_values }

journal.cli.sol.update.argument.automatic.value = { $automatic_flag } takes { $automatic_values }, not '{ $value }'

journal.cli.sol.update.argument.lane.missing = { $lane_flag } requires a value

journal.cli.sol.update.argument.lane.utf8 = { $lane_flag } must be valid { $encoding }

journal.cli.sol.update.argument.lane.value = invalid release lane '{ $lane }'; choose { $lane_values }

journal.cli.sol.update.argument.mode.conflict = choose only one of { $check_flag }, { $hold_flag }, { $unhold_flag }, or { $rollback_flag }

journal.cli.sol.update.argument.unknown = unrecognized flag: { $flag }

journal.cli.sol.update.argument.utf8 = arguments must be valid { $encoding }

journal.cli.sol.update.argument.version.conflict = { $version_flag } cannot be combined with { $check_flag }, { $hold_flag }, or { $unhold_flag }

journal.cli.sol.update.argument.version.format = invalid version format '{ $version }'; must be { $version_format } or { $revision_format }

journal.cli.sol.update.argument.version.missing = { $version_flag } requires a value

journal.cli.sol.update.argument.version.numeric = invalid version format '{ $version }'; components must be numeric

journal.cli.sol.update.argument.version.revision = invalid platform revision in '{ $version }'; revision must be numeric digits

journal.cli.sol.update.argument.version.utf8 = { $version_flag } must be valid { $encoding }

journal.cli.sol.update.argument.yes.conflict = { $yes_flag } cannot be combined with { $check_flag }, { $hold_flag }, or { $unhold_flag }

journal.cli.sol.update.automatic.choice = couldn't save your choice.

journal.cli.sol.update.automatic.home = couldn't find your data directory.

journal.cli.sol.update.automatic.ineligible = this install can't update itself. automatic updates work on an install from { $install_url }.

journal.cli.sol.update.automatic.ineligible.generic = this install can't update itself.

journal.cli.sol.update.automatic.linux = { $automatic_command } works on linux.

journal.cli.sol.update.automatic.off = automatic updates are off. to update, run { $update_command }.

journal.cli.sol.update.automatic.on = automatic updates are on. your journal never restarts on its own; a new version starts the next time it does.

journal.cli.sol.update.automatic.package.add = installed from a package. to get updates through your system's updater, add the { $brand } repository: { $package_url }

journal.cli.sol.update.automatic.prefix = the install directory's name can't go in a service file.

journal.cli.sol.update.automatic.run = { $result ->
    [up-to-date] automatic update: up to date
    [installed] automatic update: installed a new version
    [off] automatic update: automatic updates are off
    [paused] automatic update: stopped after two tries that didn't work
    [held] automatic update: updates are held
    [hold-unknown] automatic update: the update hold can't be read
    [offline] automatic update: couldn't reach the update service
    [metered] automatic update: waiting for a connection that isn't metered
    [waiting-for-signed-installer] automatic update: waiting for the next release
    [busy] automatic update: another update was running
    [not-eligible] automatic update: this install can't update itself
    *[failed] automatic update: the update didn't work ({ $code })
    }

journal.cli.sol.update.automatic.session = couldn't set up the daily update check. your systemd user session may not be running.

journal.cli.sol.update.automatic.status = { $choice ->
    [on] automatic updates: on
    [off] automatic updates: off
    [unasked] automatic updates: not turned on
    *[paused] automatic updates: stopped after two tries that didn't work
    }

journal.cli.sol.update.automatic.status.checked = { $choice ->
    [on] automatic updates: on · last checked { $date }
    [off] automatic updates: off · last checked { $date }
    [unasked] automatic updates: not turned on · last checked { $date }
    *[paused] automatic updates: stopped after two tries that didn't work · last checked { $date }
    }

journal.cli.sol.update.automatic.status.off.result = { $result ->
    [up-to-date] automatic updates: off · up to date
    [installed] automatic updates: off · installed a new version
    [off] automatic updates: off · automatic updates are off
    [paused] automatic updates: off · stopped after two tries that didn't work
    [held] automatic updates: off · updates are held
    [hold-unknown] automatic updates: off · the update hold can't be read
    [offline] automatic updates: off · couldn't reach the update service
    [metered] automatic updates: off · waiting for a connection that isn't metered
    [waiting-for-signed-installer] automatic updates: off · waiting for the next release
    [busy] automatic updates: off · another update was running
    [not-eligible] automatic updates: off · this install can't update itself
    *[failed] automatic updates: off · the update didn't work ({ $code })
    }

journal.cli.sol.update.automatic.status.off.result.checked = { $result ->
    [up-to-date] automatic updates: off · last checked { $date } · up to date
    [installed] automatic updates: off · last checked { $date } · installed a new version
    [off] automatic updates: off · last checked { $date } · automatic updates are off
    [paused] automatic updates: off · last checked { $date } · stopped after two tries that didn't work
    [held] automatic updates: off · last checked { $date } · updates are held
    [hold-unknown] automatic updates: off · last checked { $date } · the update hold can't be read
    [offline] automatic updates: off · last checked { $date } · couldn't reach the update service
    [metered] automatic updates: off · last checked { $date } · waiting for a connection that isn't metered
    [waiting-for-signed-installer] automatic updates: off · last checked { $date } · waiting for the next release
    [busy] automatic updates: off · last checked { $date } · another update was running
    [not-eligible] automatic updates: off · last checked { $date } · this install can't update itself
    *[failed] automatic updates: off · last checked { $date } · the update didn't work ({ $code })
    }

journal.cli.sol.update.automatic.status.on.result = { $result ->
    [up-to-date] automatic updates: on · up to date
    [installed] automatic updates: on · installed a new version
    [off] automatic updates: on · automatic updates are off
    [paused] automatic updates: on · stopped after two tries that didn't work
    [held] automatic updates: on · updates are held
    [hold-unknown] automatic updates: on · the update hold can't be read
    [offline] automatic updates: on · couldn't reach the update service
    [metered] automatic updates: on · waiting for a connection that isn't metered
    [waiting-for-signed-installer] automatic updates: on · waiting for the next release
    [busy] automatic updates: on · another update was running
    [not-eligible] automatic updates: on · this install can't update itself
    *[failed] automatic updates: on · the update didn't work ({ $code })
    }

journal.cli.sol.update.automatic.status.on.result.checked = { $result ->
    [up-to-date] automatic updates: on · last checked { $date } · up to date
    [installed] automatic updates: on · last checked { $date } · installed a new version
    [off] automatic updates: on · last checked { $date } · automatic updates are off
    [paused] automatic updates: on · last checked { $date } · stopped after two tries that didn't work
    [held] automatic updates: on · last checked { $date } · updates are held
    [hold-unknown] automatic updates: on · last checked { $date } · the update hold can't be read
    [offline] automatic updates: on · last checked { $date } · couldn't reach the update service
    [metered] automatic updates: on · last checked { $date } · waiting for a connection that isn't metered
    [waiting-for-signed-installer] automatic updates: on · last checked { $date } · waiting for the next release
    [busy] automatic updates: on · last checked { $date } · another update was running
    [not-eligible] automatic updates: on · last checked { $date } · this install can't update itself
    *[failed] automatic updates: on · last checked { $date } · the update didn't work ({ $code })
    }

journal.cli.sol.update.automatic.status.paused.result = { $result ->
    [up-to-date] automatic updates: stopped after two tries that didn't work · up to date
    [installed] automatic updates: stopped after two tries that didn't work · installed a new version
    [off] automatic updates: stopped after two tries that didn't work · automatic updates are off
    [paused] automatic updates: stopped after two tries that didn't work · stopped after two tries that didn't work
    [held] automatic updates: stopped after two tries that didn't work · updates are held
    [hold-unknown] automatic updates: stopped after two tries that didn't work · the update hold can't be read
    [offline] automatic updates: stopped after two tries that didn't work · couldn't reach the update service
    [metered] automatic updates: stopped after two tries that didn't work · waiting for a connection that isn't metered
    [waiting-for-signed-installer] automatic updates: stopped after two tries that didn't work · waiting for the next release
    [busy] automatic updates: stopped after two tries that didn't work · another update was running
    [not-eligible] automatic updates: stopped after two tries that didn't work · this install can't update itself
    *[failed] automatic updates: stopped after two tries that didn't work · the update didn't work ({ $code })
    }

journal.cli.sol.update.automatic.status.paused.result.checked = { $result ->
    [up-to-date] automatic updates: stopped after two tries that didn't work · last checked { $date } · up to date
    [installed] automatic updates: stopped after two tries that didn't work · last checked { $date } · installed a new version
    [off] automatic updates: stopped after two tries that didn't work · last checked { $date } · automatic updates are off
    [paused] automatic updates: stopped after two tries that didn't work · last checked { $date } · stopped after two tries that didn't work
    [held] automatic updates: stopped after two tries that didn't work · last checked { $date } · updates are held
    [hold-unknown] automatic updates: stopped after two tries that didn't work · last checked { $date } · the update hold can't be read
    [offline] automatic updates: stopped after two tries that didn't work · last checked { $date } · couldn't reach the update service
    [metered] automatic updates: stopped after two tries that didn't work · last checked { $date } · waiting for a connection that isn't metered
    [waiting-for-signed-installer] automatic updates: stopped after two tries that didn't work · last checked { $date } · waiting for the next release
    [busy] automatic updates: stopped after two tries that didn't work · last checked { $date } · another update was running
    [not-eligible] automatic updates: stopped after two tries that didn't work · last checked { $date } · this install can't update itself
    *[failed] automatic updates: stopped after two tries that didn't work · last checked { $date } · the update didn't work ({ $code })
    }

journal.cli.sol.update.automatic.status.unasked.result = { $result ->
    [up-to-date] automatic updates: not turned on · up to date
    [installed] automatic updates: not turned on · installed a new version
    [off] automatic updates: not turned on · automatic updates are off
    [paused] automatic updates: not turned on · stopped after two tries that didn't work
    [held] automatic updates: not turned on · updates are held
    [hold-unknown] automatic updates: not turned on · the update hold can't be read
    [offline] automatic updates: not turned on · couldn't reach the update service
    [metered] automatic updates: not turned on · waiting for a connection that isn't metered
    [waiting-for-signed-installer] automatic updates: not turned on · waiting for the next release
    [busy] automatic updates: not turned on · another update was running
    [not-eligible] automatic updates: not turned on · this install can't update itself
    *[failed] automatic updates: not turned on · the update didn't work ({ $code })
    }

journal.cli.sol.update.automatic.status.unasked.result.checked = { $result ->
    [up-to-date] automatic updates: not turned on · last checked { $date } · up to date
    [installed] automatic updates: not turned on · last checked { $date } · installed a new version
    [off] automatic updates: not turned on · last checked { $date } · automatic updates are off
    [paused] automatic updates: not turned on · last checked { $date } · stopped after two tries that didn't work
    [held] automatic updates: not turned on · last checked { $date } · updates are held
    [hold-unknown] automatic updates: not turned on · last checked { $date } · the update hold can't be read
    [offline] automatic updates: not turned on · last checked { $date } · couldn't reach the update service
    [metered] automatic updates: not turned on · last checked { $date } · waiting for a connection that isn't metered
    [waiting-for-signed-installer] automatic updates: not turned on · last checked { $date } · waiting for the next release
    [busy] automatic updates: not turned on · last checked { $date } · another update was running
    [not-eligible] automatic updates: not turned on · last checked { $date } · this install can't update itself
    *[failed] automatic updates: not turned on · last checked { $date } · the update didn't work ({ $code })
    }

journal.cli.sol.update.catalogue.download = could not fetch the release catalogue.

journal.cli.sol.update.catalogue.key = the release key in this build is unreadable.

journal.cli.sol.update.catalogue.lane = unknown release lane: { $lane }

journal.cli.sol.update.catalogue.latest = the latest release marker is malformed.

journal.cli.sol.update.catalogue.pin = the release catalogue names a different release key.

journal.cli.sol.update.catalogue.release = the release catalogue does not match the requested release.

journal.cli.sol.update.catalogue.schema.bad.catalogue.revision = the release catalogue is invalid: bad catalogue revision.

journal.cli.sol.update.catalogue.schema.bad.desktop.version = the release catalogue is invalid: bad desktop version.

journal.cli.sol.update.catalogue.schema.bad.installer = the release catalogue is invalid: bad installer.

journal.cli.sol.update.catalogue.schema.bad.version = the release catalogue is invalid: bad version.

journal.cli.sol.update.catalogue.schema.installer.in.an.unsupported.schema = the release catalogue is invalid: installer in an unsupported schema.

journal.cli.sol.update.catalogue.schema.key.id = the release catalogue is invalid: key id.

journal.cli.sol.update.catalogue.schema.no.journal.version = the release catalogue is invalid: no journal version.

journal.cli.sol.update.catalogue.schema.no.version = the release catalogue is invalid: no version.

journal.cli.sol.update.catalogue.schema.not.json = the release catalogue is invalid: not json.

journal.cli.sol.update.catalogue.schema.unsupported.protocol = the release catalogue is invalid: unsupported protocol.

journal.cli.sol.update.catalogue.schema.unsupported.schema = the release catalogue is invalid: unsupported schema.

journal.cli.sol.update.catalogue.signature = the release catalogue failed its signature check.

journal.cli.sol.update.catalogue.version = the requested platform version is invalid.

journal.cli.sol.update.help = usage: { $syntax }{ $line_break }{ $line_break }update your installed { $brand } journal and apps.{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $check_option }{ $gap_6 }check whether an update is available without changing anything{ $line_break }{ $indent_2 }{ $json_option }{ $gap_7 }emit output as json{ $line_break }{ $indent_2 }{ $lane_option }{ $gap_7 }release lane to track ({ $lane_values }){ $line_break }{ $indent_2 }{ $version_option }{ $gap_4 }specific target version to install{ $line_break }{ $indent_2 }{ $hold_option }{ $gap_7 }hold updates so a later update does not install them{ $line_break }{ $indent_2 }{ $unhold_option }{ $gap_5 }release update hold{ $line_break }{ $indent_2 }{ $rollback_option }{ $gap_3 }roll back to an earlier retained installation{ $line_break }{ $indent_2 }{ $yes_option }{ $gap_8 }accept unattended update prompts{ $line_break }{ $indent_2 }{ $automatic_option }{ $gap_2 }{ $automatic_values }: whether your journal updates itself once a day{ $line_break }{ $indent_2 }{ $help_option }{ $gap_7 }show this help message

journal.cli.sol.update.receipt.claimed.twice = component { $component } claimed by both tree and package receipts

journal.cli.sol.update.receipt.duplicate.roles = both journal and cli components are recorded

journal.cli.sol.update.receipt.incomplete = the installation receipt is incomplete or unsupported. use the original installer to recover it.

journal.cli.sol.update.receipt.key.duplicate = duplicate key { $key }

journal.cli.sol.update.receipt.key.outside = key outside section

journal.cli.sol.update.receipt.line.invalid = malformed line: { $line }

journal.cli.sol.update.receipt.line.pair = not key=value

journal.cli.sol.update.receipt.native.file = native { $receipt_file } is not a regular file

journal.cli.sol.update.receipt.native.malformed = malformed native { $receipt_file }: { $detail }

journal.cli.sol.update.receipt.native.route = native receipt route must be tree

journal.cli.sol.update.receipt.native.schema = native receipt { $schema_field } must be 1

journal.cli.sol.update.receipt.native.version = installed version does not match receipt { $journal_field }

journal.cli.sol.update.receipt.package.file = package { $config_file } is not a regular file

journal.cli.sol.update.receipt.package.malformed = malformed package { $config_file }: { $detail }

journal.cli.sol.update.receipt.section.duplicate = duplicate section [{ $section }]

journal.cli.sol.update.receipt.section.key.duplicate = duplicate key { $key } in section [{ $section }]

journal.cli.sol.update.receipt.section.unclosed = unclosed section header

journal.cli.sol.update.receipt.section.unknown = unrecognized section in { $config_file }: { $section }

journal.cli.sol.update.receipt.unrecorded = this journal was installed from a package without the { $brand } installer, so { $update_command } can't update it yet. run { $install_command } once. it checks the installed files against the signed release, and { $update_command } updates it after that.

journal.cli.sol.update.receipt.user.file = user { $config_file } is not a regular file

journal.cli.sol.update.receipt.user.malformed = malformed user { $config_file }: { $detail }

journal.cli.sol.update.report.available = an update is available.

journal.cli.sol.update.report.available.package.add = an update is available. installed from a package. to get updates through your system's updater, add the { $brand } repository: { $package_url }

journal.cli.sol.update.report.available.package.configured = an update is available. installed from a package. updates come through your system's updater.

journal.cli.sol.update.report.catalogue.invalid = the selected release metadata is invalid.

journal.cli.sol.update.report.checkout = this copy is a source checkout. install the released journal to update it.

journal.cli.sol.update.report.current = your journal is up to date.

journal.cli.sol.update.report.current.package.add = your journal is up to date. installed from a package. to get updates through your system's updater, add the { $brand } repository: { $package_url }

journal.cli.sol.update.report.current.package.configured = your journal is up to date. installed from a package. updates come through your system's updater.

journal.cli.sol.update.report.held = updates are held. run { $unhold_command } to release them.

journal.cli.sol.update.report.hold.changed = the update hold cannot be read. nothing was changed.

journal.cli.sol.update.report.hold.off = updates are no longer held.

journal.cli.sol.update.report.hold.on = updates are held.

journal.cli.sol.update.report.hold.save = could not save the update hold. check your data directory permissions.

journal.cli.sol.update.report.hold.unreadable = the update hold cannot be read.

journal.cli.sol.update.report.install.directory = the installation directory could not be identified.

journal.cli.sol.update.report.installer.arguments = invalid installer arguments.

journal.cli.sol.update.report.installer.failed = installation did not succeed. use the original installer to recover.

journal.cli.sol.update.report.installer.fetch = could not fetch the installer.

journal.cli.sol.update.report.installer.run = could not run the installer.

journal.cli.sol.update.report.installer.tookover = updated your journal. it now updates with the { $brand } installer, from where it is.

journal.cli.sol.update.report.installer.updated = updated your journal.

journal.cli.sol.update.report.linux = { $update_command } is available on linux.

journal.cli.sol.update.report.mode = this mode is not an update.

journal.cli.sol.update.report.owner.unknown = could not identify how this copy of { $brand } was installed. use the installer that installed it to update it.

journal.cli.sol.update.report.platform.older = could not match this older build to a platform release. choose a platform version with { $version_flag }.

journal.cli.sol.update.report.rollback.package = this package route cannot install an older journal. nothing was changed.

journal.cli.sol.update.report.unknown = update status is unknown.

journal.cli.sol.update.rollback.epoch = install the current version or another release in this upgrade epoch

journal.cli.sol.update.rollback.link = current version link is missing or unreadable

journal.cli.sol.update.rollback.metadata = installed release metadata is unreadable

journal.cli.sol.update.rollback.no.older = no eligible older build is retained. choose a platform version with { $version_flag }, or reinstall the current release.

journal.cli.sol.update.rollback.policy = the current release has no supported retention policy

journal.cli.sol.update.rollback.range.absent = the installed release has no declared reader range. nothing was changed.

journal.cli.sol.update.rollback.range.outside = the selected version is outside the declared reader range. nothing was changed.

journal.cli.sol.update.rollback.retained = that build is not retained; reinstall the current version

journal.cli.sol.update.rollback.state = the installer cannot establish the state of your journal. nothing was changed.

journal.cli.sol.update.rollback.version = target version { $target_version } is not older than { $installed_version }

journal.cli.sol.update.summary.failure = { $code }: { $message }

journal.cli.sol.update.summary.hold.held = hold: held

journal.cli.sol.update.summary.hold.unheld = hold: unheld

journal.cli.sol.update.summary.hold.unknown = hold: unknown

journal.cli.sol.update.summary.journal = journal: { $value }

journal.cli.sol.update.summary.lane = lane: { $value }

journal.cli.sol.update.summary.platform = platform: { $value }

journal.cli.sol.update.summary.route = route: { $value }

journal.cli.sol.update.summary.target = available: { $available } (journal { $journal })

journal.cli.sol.usage = Usage: { $syntax }

journal.cli.journal.archive.argument = unexpected argument

journal.cli.journal.archive.day.exclusive = { $day_flag } cannot be combined with { $from_flag } or { $to_flag }

journal.cli.journal.archive.day.invalid = { $token_day } must be a real { $token_yyyymmdd } date

journal.cli.journal.archive.day.required = { $day_flag } requires { $token_yyyymmdd }

journal.cli.journal.archive.encode.finish.body = archive finish body: { $detail }

journal.cli.journal.archive.encode.finish.finalize = archive finish finalize: { $detail }

journal.cli.journal.archive.encode.member = archive write { $member }: { $detail }

journal.cli.journal.archive.encode.metadata = invalid archive metadata { $field } { $value }: { $reason }

journal.cli.journal.archive.encode.source = archive source: { $detail }

journal.cli.journal.archive.encode.write = archive write: { $detail }

journal.cli.journal.archive.encode.writer = invalid archive writer: { $reason }

journal.cli.journal.archive.export.help = Usage: { $syntax }{ $line_break }{ $line_break }create a { $token_zip } of your journal, including settings and credentials.{ $line_break }processing logs, rebuildable indexes and caches, locks and temporary files are left out.{ $line_break }with a date filter, only the selected days are included.

journal.cli.journal.archive.exports.unsafe = unsafe exports directory: { $path }

journal.cli.journal.archive.fact.archive.output.contains.a.null.byte = archive output contains a null byte

journal.cli.journal.archive.fact.archive.output.ends.with.a.separator = archive output ends with a separator

journal.cli.journal.archive.fact.archive.output.has.no.file.name = archive output has no file name

journal.cli.journal.archive.fact.archive.output.has.no.parent = archive output has no parent

journal.cli.journal.archive.fact.archive.output.must.be.one.file.name = archive output must be one file name

journal.cli.journal.archive.fact.archive.output.parent.is.missing.or.unsafe = archive output parent is missing or unsafe

journal.cli.journal.archive.fact.archive.output.path.is.not.valid.utf8 = archive output path is not valid { $token_utf_8 }

journal.cli.journal.archive.fact.archive.output.uses.an.unsupported.prefix = archive output uses an unsupported prefix

journal.cli.journal.archive.fact.archive.parent.component.is.invalid = archive parent component is invalid

journal.cli.journal.archive.fact.block.device = block device

journal.cli.journal.archive.fact.character.device = character device

journal.cli.journal.archive.fact.could.not.determine.writer.position = could not determine writer position

journal.cli.journal.archive.fact.could.not.read.writer.flags = could not read writer flags

journal.cli.journal.archive.fact.could.not.stat.writer = could not stat writer

journal.cli.journal.archive.fact.directory = directory

journal.cli.journal.archive.fact.fifo = fifo

journal.cli.journal.archive.fact.filesystem.root.is.not.a.directory = filesystem root is not a directory

journal.cli.journal.archive.fact.has.no.parent.directory = has no parent directory

journal.cli.journal.archive.fact.must.be.a.regular.file = must be a regular file

journal.cli.journal.archive.fact.must.be.empty = must be empty

journal.cli.journal.archive.fact.must.be.opened.for.writing = must be opened for writing

journal.cli.journal.archive.fact.must.be.positioned.at.the.start = must be positioned at the start

journal.cli.journal.archive.fact.must.be.valid.utf8 = must be valid { $token_utf_8 }

journal.cli.journal.archive.fact.must.name.one.normal.file.leaf = must name one normal file leaf

journal.cli.journal.archive.fact.must.not.be.opened.in.append.mode = must not be opened in append mode

journal.cli.journal.archive.fact.must.resolve.to.an.absolute.path = must resolve to an absolute path

journal.cli.journal.archive.fact.other = other

journal.cli.journal.archive.fact.regular.file = regular file

journal.cli.journal.archive.fact.relative.archive.output.requires.an.absolute.cwd = relative archive output requires an absolute cwd

journal.cli.journal.archive.fact.socket = socket

journal.cli.journal.archive.fact.symlink = symlink

journal.cli.journal.archive.fact.utf8.member.path.exceeds.zip.u16.byte.limit = { $token_utf_8 } member path exceeds { $token_zip } { $token_u16 } byte limit

journal.cli.journal.archive.from.invalid = { $token_from } must be a real { $token_yyyymmdd } date

journal.cli.journal.archive.from.required = { $from_flag } requires { $token_yyyymmdd }

journal.cli.journal.archive.import.ambiguities = kind mismatch at { $token_entities_ambiguities_jsonl }

journal.cli.journal.archive.import.appeared = { $relative } appeared while the import was running

journal.cli.journal.archive.import.busy = archive merge lock is busy at { $path }: { $detail }

journal.cli.journal.archive.import.cleanup = failed cleaning extraction { $path }: { $detail }

journal.cli.journal.archive.import.compressed = { $count ->
    [one] archive { $path } is { $bytes } byte; maximum is { $maximum }
   *[other] archive { $path } is { $bytes } bytes; maximum is { $maximum }
    }

journal.cli.journal.archive.import.coordinate = invalid planned recording coordinate

journal.cli.journal.archive.import.directory.remains = directory remains

journal.cli.journal.archive.import.encrypted.extraction = encrypted archive entry during extraction: { $entry }

journal.cli.journal.archive.import.encrypted.validation = encrypted archive entry during validation: { $entry }

journal.cli.journal.archive.import.entity = entity merge failed for { $entity_id }: { $detail }

journal.cli.journal.archive.import.expanded = { $count ->
    [one] archive expands to { $bytes } byte; maximum is { $maximum }
   *[other] archive expands to { $bytes } bytes; maximum is { $maximum }
    }

journal.cli.journal.archive.import.extract = failed extracting { $archive } into { $dir }: { $detail }

journal.cli.journal.archive.import.facet = facet merge failed for { $facet }: { $detail }

journal.cli.journal.archive.import.handling = while handling { $path }: { $detail }

journal.cli.journal.archive.import.hidden.archive = this archive's hidden days of body data can't be read, so this archive can't be imported

journal.cli.journal.archive.import.hidden.details = { $detail }; { $warning }

journal.cli.journal.archive.import.hidden.target = this journal can't read which days of body data you hid, so this archive can't be imported

journal.cli.journal.archive.import.hidden.warning = { $days ->
    [one] 1 day of body data was hidden in the archive, so it's hidden in this journal too; you can unhide it on the body page.
   *[other] { $days } days of body data were hidden in the archive, so they're hidden in this journal too; you can unhide them on the body page.
    }

journal.cli.journal.archive.import.invalid = invalid archive { $path }: { $detail }

journal.cli.journal.archive.import.kind = kind mismatch at { $relative }

journal.cli.journal.archive.import.legacy = a { $token_tgz } or { $token_tar_gz } archive isn't supported. if it's an old transfer archive, export that day again as a { $token_zip } from the journal it came from and merge that, or merge this file into a new, empty journal on an earlier { $token_2_x } release, export that journal as a { $token_zip }, and merge that { $token_zip } into your journal

journal.cli.journal.archive.import.lock = archive merge lock failed at { $path }: { $detail }

journal.cli.journal.archive.import.log = decision log write failed at { $path }: { $detail }

journal.cli.journal.archive.import.magic = unrecognized archive magic

journal.cli.journal.archive.import.marker = stream marker update failed after archive content was published: { $detail }; published content was not rolled back

journal.cli.journal.archive.import.marker.day = { $day }: { $detail }

journal.cli.journal.archive.import.merge = import merge failed at { $path }: { $detail }

journal.cli.journal.archive.import.missing = archive not found: { $path }

journal.cli.journal.archive.import.name = non-{ $token_utf_8 } or missing path name

journal.cli.journal.archive.import.owner = wait for the holder or inspect/remove only verified-stale owner metadata at { $path }

journal.cli.journal.archive.import.path.encoding = { $path } is not { $token_utf_8 }

journal.cli.journal.archive.import.path.outside = { $path } is outside the journal

journal.cli.journal.archive.import.publish = archive merge publish failed: { $detail }

journal.cli.journal.archive.import.redirect.entity = this journal merged '{ $source_id }' into '{ $survivor }', and merges can't be undone, so this archive can't be imported

journal.cli.journal.archive.import.redirect.facet = this journal had a facet named '{ $facet }' that was merged or renamed into another, and that name leads there now, so this archive can't be imported

journal.cli.journal.archive.import.segment = segment merge failed at { $path }: { $detail }

journal.cli.journal.archive.import.segment.appeared = segment appeared before archive publication

journal.cli.journal.archive.import.segment.changed = published archive segment changed

journal.cli.journal.archive.import.segment.symlink = symbolic link in segment

journal.cli.journal.archive.import.space = insufficient free space at { $path }: { $available } available, { $required } required

journal.cli.journal.archive.import.stage = staging write failed at { $path }: { $detail }

journal.cli.journal.archive.import.stage.changed = archive staging parent changed

journal.cli.journal.archive.import.stopped = import stopped

journal.cli.journal.archive.import.symlink = symbolic link

journal.cli.journal.archive.import.traversal = absolute or traversal path

journal.cli.journal.archive.import.unambiguous = archive has no unambiguous journal root

journal.cli.journal.archive.import.undo = { $detail }; undo incomplete: { $failures }

journal.cli.journal.archive.import.undo.coordinate = coordinate { $relative }: { $detail }

journal.cli.journal.archive.import.undo.restore = restore { $relative }: { $detail }

journal.cli.journal.archive.import.undo.rewrite = rewrite identity map: { $detail }

journal.cli.journal.archive.import.undo.unlink = unlink { $relative }: { $detail }

journal.cli.journal.archive.import.unimplemented = import-sources: unimplemented: { $module }

journal.cli.journal.archive.import.unsafe.extraction = unsafe archive entry during extraction: { $entry }: { $detail }

journal.cli.journal.archive.import.unsafe.validation = unsafe archive entry during validation: { $entry }: { $detail }

journal.cli.journal.archive.incomplete = { $committed } committed, { $skipped } skipped, { $failed } failed

journal.cli.journal.archive.ios = unavailable on { $token_ios }

journal.cli.journal.archive.merge.help = Usage: { $syntax }

journal.cli.journal.archive.metadata.reason.invalid.calendar.date.or.time = invalid calendar date or time

journal.cli.journal.archive.metadata.reason.invalid.day = invalid day

journal.cli.journal.archive.metadata.reason.invalid.hour = invalid hour

journal.cli.journal.archive.metadata.reason.invalid.minute = invalid minute

journal.cli.journal.archive.metadata.reason.invalid.month = invalid month

journal.cli.journal.archive.metadata.reason.invalid.second = invalid second

journal.cli.journal.archive.metadata.reason.must.contain.digits.in.numeric.positions = must contain digits in numeric positions

journal.cli.journal.archive.metadata.reason.must.not.be.empty = must not be empty

journal.cli.journal.archive.metadata.reason.must.not.contain.ascii.control.characters = must not contain ASCII control characters

journal.cli.journal.archive.metadata.reason.must.use.yyyy.mm.ddthh.mm.ssz = must use YYYY-MM-DDTHH:MM:SSZ

journal.cli.journal.archive.metadata.reason.second.must.be.in.seconds = second must be in 0..=59

journal.cli.journal.archive.metadata.reason.year.must.be.in.years = year must be in 1980..=2107

journal.cli.journal.archive.negative.identity = source identity is negative

journal.cli.journal.archive.negative.size = regular-file size is negative

journal.cli.journal.archive.out.required = { $out_flag } requires { $token_path }

journal.cli.journal.archive.output.parent = archive output has no parent

journal.cli.journal.archive.output.tree = output is inside exported { $family } tree

journal.cli.journal.archive.preview = Archive merge (days: { $days }): 0 committed, { $skipped } skipped, 0 failed.

journal.cli.journal.archive.publish.cleanup = clean archive temporary file: { $detail }

journal.cli.journal.archive.publish.createtemp = create archive temporary file: { $detail }

journal.cli.journal.archive.publish.publish = publish archive: { $detail }

journal.cli.journal.archive.publish.syncdirectory = sync archive directory: { $detail }

journal.cli.journal.archive.publish.synctemp = sync archive temporary file: { $detail }

journal.cli.journal.archive.range = { $from_flag } must not be after { $to_flag }

journal.cli.journal.archive.root.name = journal root has no file name

journal.cli.journal.archive.root.parent = journal root has no parent

journal.cli.journal.archive.scratch.name = could not allocate an exclusive archive scratch file

journal.cli.journal.archive.skipped = { $token_solstone_journal_archive_export }: skipped top-level entries: { $skipped }

journal.cli.journal.archive.source.budget = unsupported journal root { $path }: archive source exceeds its supported inventory budget

journal.cli.journal.archive.source.changed = journal source changed at { $member }

journal.cli.journal.archive.source.io.list.journal.directory = list journal directory: { $detail }

journal.cli.journal.archive.source.io.list.journal.directory.member = list journal directory { $member }: { $detail }

journal.cli.journal.archive.source.io.member = { $operation } { $member }: { $detail }

journal.cli.journal.archive.source.io.open.journal.directory.for.listing = open journal directory for listing: { $detail }

journal.cli.journal.archive.source.io.open.journal.directory.for.listing.member = open journal directory for listing { $member }: { $detail }

journal.cli.journal.archive.source.io.open.journal.directory.member = open journal directory { $member }: { $detail }

journal.cli.journal.archive.source.io.open.journal.file = open journal file: { $detail }

journal.cli.journal.archive.source.io.open.journal.file.member = open journal file { $member }: { $detail }

journal.cli.journal.archive.source.io.open.retained.journal.root = open retained journal root: { $detail }

journal.cli.journal.archive.source.io.open.retained.journal.root.member = open retained journal root { $member }: { $detail }

journal.cli.journal.archive.source.io.read.inventoried.file = read inventoried file: { $detail }

journal.cli.journal.archive.source.io.read.inventoried.file.member = read inventoried file { $member }: { $detail }

journal.cli.journal.archive.source.io.read.regular.file.size = read regular-file size: { $detail }

journal.cli.journal.archive.source.io.read.regular.file.size.member = read regular-file size { $member }: { $detail }

journal.cli.journal.archive.source.io.read.source.file.identity.member = read source file identity { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.directory = stat inventoried directory: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.directory.member = stat inventoried directory { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.file = stat inventoried file: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.file.member = stat inventoried file { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.journal.entry = stat journal entry: { $detail }

journal.cli.journal.archive.source.io.stat.journal.entry.member = stat journal entry { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.directory = stat opened inventoried directory: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.directory.member = stat opened inventoried directory { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.file = stat opened inventoried file: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.file.member = stat opened inventoried file { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.directory = stat opened journal directory: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.directory.member = stat opened journal directory { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.file = stat opened journal file: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.file.member = stat opened journal file { $member }: { $detail }

journal.cli.journal.archive.source.missing = { $token_source } does not exist: { $path }

journal.cli.journal.archive.source.regular = { $token_source } must be a regular file

journal.cli.journal.archive.source.required = { $token_source } is required

journal.cli.journal.archive.source.unsafe = unsafe journal entry { $member }: { $kind }

journal.cli.journal.archive.source.windows.arm.recursive.namespace.witness = arm recursive namespace witness: { $detail }

journal.cli.journal.archive.source.windows.arm.recursive.namespace.witness.member = arm recursive namespace witness { $member }: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root = borrow admitted journal root: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.member = borrow admitted journal root { $member }: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.namespace.watch = borrow admitted journal-root namespace watch: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.namespace.watch.member = borrow admitted journal-root namespace watch { $member }: { $detail }

journal.cli.journal.archive.source.windows.cancel.recursive.namespace.witness = cancel recursive namespace witness: { $detail }

journal.cli.journal.archive.source.windows.cancel.recursive.namespace.witness.member = cancel recursive namespace witness { $member }: { $detail }

journal.cli.journal.archive.source.windows.check.recursive.namespace.witness = check recursive namespace witness: { $detail }

journal.cli.journal.archive.source.windows.check.recursive.namespace.witness.member = check recursive namespace witness { $member }: { $detail }

journal.cli.journal.archive.source.windows.create.recursive.namespace.witness.event = create recursive namespace witness event: { $detail }

journal.cli.journal.archive.source.windows.create.recursive.namespace.witness.event.member = create recursive namespace witness event { $member }: { $detail }

journal.cli.journal.archive.source.windows.drain.recursive.namespace.witness = drain recursive namespace witness: { $detail }

journal.cli.journal.archive.source.windows.drain.recursive.namespace.witness.member = drain recursive namespace witness { $member }: { $detail }

journal.cli.journal.archive.source.windows.list.retained.directory = list retained directory: { $detail }

journal.cli.journal.archive.source.windows.list.retained.directory.member = list retained directory { $member }: { $detail }

journal.cli.journal.archive.source.windows.open.retained.relative.entry = open retained relative entry: { $detail }

journal.cli.journal.archive.source.windows.open.retained.relative.entry.member = open retained relative entry { $member }: { $detail }

journal.cli.journal.archive.source.windows.parse.retained.directory.listing = parse retained directory listing: { $detail }

journal.cli.journal.archive.source.windows.parse.retained.directory.listing.member = parse retained directory listing { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.attributes = query retained entry attributes: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.attributes.member = query retained entry attributes { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.identity = query retained entry identity: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.identity.member = query retained entry identity { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.metadata = query retained entry metadata: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.metadata.member = query retained entry metadata { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.type = query retained entry type: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.type.member = query retained entry type { $member }: { $detail }

journal.cli.journal.archive.source.windows.read.observed.file = read observed file: { $detail }

journal.cli.journal.archive.source.windows.read.observed.file.member = read observed file { $member }: { $detail }

journal.cli.journal.archive.source.windows.reverify.retained.relative.directory.identity = reverify retained relative directory identity: { $detail }

journal.cli.journal.archive.source.windows.reverify.retained.relative.directory.identity.member = reverify retained relative directory identity { $member }: { $detail }

journal.cli.journal.archive.source.windows.size.checked.read.buffer = size checked-read buffer: { $detail }

journal.cli.journal.archive.source.windows.size.checked.read.buffer.member = size checked-read buffer { $member }: { $detail }

journal.cli.journal.archive.source.windows.trace.retained.relative.directory.listing.open = trace retained relative directory listing open: { $detail }

journal.cli.journal.archive.source.windows.trace.retained.relative.directory.listing.open.member = trace retained relative directory listing open { $member }: { $detail }

journal.cli.journal.archive.source.windows.unsupported.arm.recursive.namespace.witness = unsupported journal root { $path }: arm recursive namespace witness

journal.cli.journal.archive.source.windows.unsupported.cancel.recursive.namespace.witness = unsupported journal root { $path }: cancel recursive namespace witness

journal.cli.journal.archive.source.windows.unsupported.check.recursive.namespace.witness = unsupported journal root { $path }: check recursive namespace witness

journal.cli.journal.archive.source.windows.unsupported.drain.recursive.namespace.witness = unsupported journal root { $path }: drain recursive namespace witness

journal.cli.journal.archive.source.windows.unsupported.wait.for.recursive.namespace.witness = unsupported journal root { $path }: wait for recursive namespace witness

journal.cli.journal.archive.source.windows.wait.for.recursive.namespace.witness = wait for recursive namespace witness: { $detail }

journal.cli.journal.archive.source.windows.wait.for.recursive.namespace.witness.member = wait for recursive namespace witness { $member }: { $detail }

journal.cli.journal.archive.space.overflow = available disk space overflow

journal.cli.journal.archive.space.platform = disk-space inspection is unsupported on this platform

journal.cli.journal.archive.summary = Archive merge: { $committed } committed, { $skipped } skipped, { $failed } failed.

journal.cli.journal.archive.target.changed = archive output route changed during export: { $path }

journal.cli.journal.archive.target.collision = archive output already exists: { $path }

journal.cli.journal.archive.target.invalid = invalid archive output { $path }: { $reason }

journal.cli.journal.archive.target.io = { $operation } failed for { $path }: { $detail }

journal.cli.journal.archive.target.io.open.archive.output.filesystem.root = open archive output filesystem root failed for { $path }: { $detail }

journal.cli.journal.archive.target.io.open.archive.output.parent = open archive output parent failed for { $path }: { $detail }

journal.cli.journal.archive.target.io.retain.admitted.archive.output.parent = retain admitted archive output parent failed for { $path }: { $detail }

journal.cli.journal.archive.target.io.revalidate.admitted.archive.output.parent = revalidate admitted archive output parent failed for { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.admitted.archive.output.parent = stat admitted archive output parent failed for { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.archive.output.filesystem.root = stat archive output filesystem root failed for { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.archive.output.final.name = stat archive output final name failed for { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.archive.output.parent = stat archive output parent failed for { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.opened.archive.output.parent = stat opened archive output parent failed for { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.retained.archive.output.parent = stat retained archive output parent failed for { $path }: { $detail }

journal.cli.journal.archive.target.unsafe = archive output is an unsafe { $kind }: { $path }

journal.cli.journal.archive.target.windows.changed = archive output changed: { $path }

journal.cli.journal.archive.target.windows.io = archive output { $path }: { $detail }

journal.cli.journal.archive.target.windows.unsafe = unsafe archive output { $path }: { $reason }

journal.cli.journal.archive.temp.name = could not allocate a unique archive temporary name

journal.cli.journal.archive.to.invalid = { $token_to } must be a real { $token_yyyymmdd } date

journal.cli.journal.archive.to.required = { $to_flag } requires { $token_yyyymmdd }

journal.cli.journal.archive.zero.write = archive writer returned zero bytes for a nonempty write

journal.cli.journal.facet.argument.ios = unavailable on { $platform }

journal.cli.journal.facet.argument.unexpected = unexpected argument

journal.cli.journal.facet.content.unsafe = unsafe facet content: { $path }

journal.cli.journal.facet.doctor.adopted.heading = Adopted orphan facets

journal.cli.journal.facet.doctor.adoption.failed = { $destination } (adoption failed: { $error })

journal.cli.journal.facet.doctor.adoption.unmerged = { $folders_remaining ->
    [one] { $destination } (adoption failed: { $error }; { $unmerged } was not merged)
   *[other] { $destination } (adoption failed: { $error }; { $unmerged } were not merged)
    }

journal.cli.journal.facet.doctor.argument.merge = { $merge_flag } requires { $adopt_flag }

journal.cli.journal.facet.doctor.collision.heading = Regular-file collisions

journal.cli.journal.facet.doctor.empty = Nothing to report.

journal.cli.journal.facet.doctor.failed.heading = Failed orphan facets

journal.cli.journal.facet.doctor.fix.ios = { $fix_flag } is unavailable on { $platform }

journal.cli.journal.facet.doctor.folders.hidden = Folders whose names start with a dot are never facets, so these facet declarations are ignored: { $folders }

journal.cli.journal.facet.doctor.group.ambiguous = { $members }: named like { $facets }; rename the folder to match one, then run with { $adopt_flag } { $merge_flag }

journal.cli.journal.facet.doctor.group.damaged = { $members }: named like { $facets }, whose { $declaration } needs repair first

journal.cli.journal.facet.doctor.help = Usage: { $syntax }{ $line_break }{ $retire_syntax }

journal.cli.journal.facet.doctor.history.failed = action log scan failed: { $error }

journal.cli.journal.facet.doctor.history.merged = { $name } was merged into { $destination }

journal.cli.journal.facet.doctor.history.recorded = Recorded retired facet names from the action logs:

journal.cli.journal.facet.doctor.history.renamed = { $name } was renamed into { $destination }

journal.cli.journal.facet.doctor.history.reused = These facets were renamed or merged away earlier and their names later given to a new facet, so older material that names them can't be told apart. Nothing was changed: { $names }

journal.cli.journal.facet.doctor.history.unknown = The action logs mention these names but not where they went: { $names }. If material still names one, record it with { $retire_arguments } or { $deleted_flag }.

journal.cli.journal.facet.doctor.history.unrecorded = Facet names the action logs show were retired but aren't recorded (run with { $fix_flag } to record them):

journal.cli.journal.facet.doctor.leftovers.failed = merge leftovers couldn't be read: { $error }

journal.cli.journal.facet.doctor.leftovers.kept = Leftover folders from a facet merge that may hold the only copy of some files. Don't delete them; run with { $fix_flag } to move them to the trash: { $folders }

journal.cli.journal.facet.doctor.leftovers.moved = Moved leftover folders from a facet merge to the trash, as they were: { $folders }

journal.cli.journal.facet.doctor.leftovers.safe = Leftover folders from an interrupted facet merge, safe to delete once no merge is running: { $folders }

journal.cli.journal.facet.doctor.link.duplicate.open = { $facet }: '{ $id }' is linked from more than one folder ({ $folders })

journal.cli.journal.facet.doctor.link.duplicate.repaired = { $facet }: '{ $id }' was linked from more than one folder ({ $folders })

journal.cli.journal.facet.doctor.link.merged.open = { $folder_count ->
    [one] { $facet }: '{ $id }' was merged into '{ $successor }', and folder { $folders } still links it
   *[other] { $facet }: '{ $id }' was merged into '{ $successor }', and folders { $folders } still link it
    }

journal.cli.journal.facet.doctor.link.merged.repaired = { $folder_count ->
    [one] { $facet }: '{ $id }' was merged into '{ $successor }', and folder { $folders } linked it
   *[other] { $facet }: '{ $id }' was merged into '{ $successor }', and folders { $folders } linked it
    }

journal.cli.journal.facet.doctor.link.misnamed.open = { $facet }: '{ $id }' is linked from folder { $folders }, not a folder named '{ $id }'

journal.cli.journal.facet.doctor.link.misnamed.repaired = { $facet }: '{ $id }' was linked from folder { $folders }, not a folder named '{ $id }'

journal.cli.journal.facet.doctor.link.unreadable = { $facet }: folder { $folders } has a link that can't be read or used

journal.cli.journal.facet.doctor.link.unreadable.check = { $facet }: folder { $folders } has a link that can't be read or used; { $fix_flag } leaves it as it is

journal.cli.journal.facet.doctor.link.unreadable.undecided = { $facet }: folder { $folders } has a link that can't be read or used ({ $fix_flag } can't tell which entity it links, so it leaves it)

journal.cli.journal.facet.doctor.links.check = Entity links to repair (run with { $fix_flag } to repair the ones that can be):

journal.cli.journal.facet.doctor.links.check.failed = entity link check failed: { $error }

journal.cli.journal.facet.doctor.links.left = Entity links left as they are:

journal.cli.journal.facet.doctor.links.repair.failed = entity link repair failed: { $error }

journal.cli.journal.facet.doctor.links.repaired = Entity links repaired:

journal.cli.journal.facet.doctor.links.rescan = Run '{ $rescan_command }' to refresh the index.

journal.cli.journal.facet.doctor.lock.failed = the facet lock wasn't taken: { $error }

journal.cli.journal.facet.doctor.maintenance.heading = Committed merge maintenance failures

journal.cli.journal.facet.doctor.merge.collision = { $source } -> { $destination }: { $path } (kept { $retained })

journal.cli.journal.facet.doctor.merge.failed = { $source } -> { $destination } (merge failed before commit: { $error })

journal.cli.journal.facet.doctor.merge.ios = facet merging unavailable on { $platform }

journal.cli.journal.facet.doctor.merged.heading = Merged orphan facets

journal.cli.journal.facet.doctor.merged.none = No orphan facet was registered or merged.

journal.cli.journal.facet.doctor.merges.incomplete = one or more orphan facet merges committed with maintenance failures

journal.cli.journal.facet.doctor.orphans.count = { $count ->
    [one] { $count } orphan facet found. Run with { $adopt_flag } to register it.
   *[other] { $count } orphan facet(s) found. Run with { $adopt_flag } to register them.
    }

journal.cli.journal.facet.doctor.orphans.heading = Orphan facets:

journal.cli.journal.facet.doctor.orphans.held = The action logs show these facet names were retired, so their folders are not registered; run with { $fix_flag } to record them: { $folders }

journal.cli.journal.facet.doctor.orphans.none = No orphan facets found.

journal.cli.journal.facet.doctor.orphans.retired = These folders have content under facet names that were merged or renamed into another facet, so they are not registered as facets; their files are left in place: { $folders }

journal.cli.journal.facet.doctor.queue.failed = the repair finished, but its search update could not be queued: { $error }

journal.cli.journal.facet.doctor.reference = - { $name }: { $count } ({ $first } to { $last })

journal.cli.journal.facet.doctor.references.command = To make one reachable again, run: { $retire_command }

journal.cli.journal.facet.doctor.references.failed = reference scan failed: { $error }

journal.cli.journal.facet.doctor.references.missing = Segments name these facets, which don't exist and aren't recorded as retired. Agents limited to chosen facets can't see that material; everything else can:

journal.cli.journal.facet.doctor.registered.none = No orphan facet was registered.

journal.cli.journal.facet.doctor.registration.failed = registered { $slug }, but its record could not be written: { $error }

journal.cli.journal.facet.doctor.repair.failed = retired facet names could not be repaired: { $error }

journal.cli.journal.facet.doctor.repaired.count = { $repaired ->
    [one] { $repaired } orphan facet repaired. search is catching up.
   *[other] { $repaired } orphan facet(s) repaired. search is catching up.
    }

journal.cli.journal.facet.doctor.repaired.heading = Repaired orphan facets:

journal.cli.journal.facet.doctor.repairs.failed = one or more orphan facet repairs failed

journal.cli.journal.facet.doctor.repairs.failed.count = { $repaired ->
    [one] { $failed_orphans ->
        [one] { $repaired } orphan facet repaired; { $failed_orphans } orphan facet failed. Run '{ $rescan_command }' to refresh the index.
       *[other] { $repaired } orphan facet repaired; { $failed_orphans } orphan facet(s) failed. Run '{ $rescan_command }' to refresh the index.
        }
   *[other] { $failed_orphans ->
        [one] { $repaired } orphan facet(s) repaired; { $failed_orphans } orphan facet failed. Run '{ $rescan_command }' to refresh the index.
       *[other] { $repaired } orphan facet(s) repaired; { $failed_orphans } orphan facet(s) failed. Run '{ $rescan_command }' to refresh the index.
        }
    }

journal.cli.journal.facet.doctor.repairs.maintenance.count = { $repaired ->
    [one] { $merges ->
        [one] { $repaired } orphan facet repaired; { $merges } merge committed but reported a maintenance failure after commit. See 'Committed merge maintenance failures' above. Run '{ $rescan_command }' to refresh the index.
       *[other] { $repaired } orphan facet repaired; { $merges } merge(s) committed but reported a maintenance failure after commit. See 'Committed merge maintenance failures' above. Run '{ $rescan_command }' to refresh the index.
        }
   *[other] { $merges ->
        [one] { $repaired } orphan facet(s) repaired; { $merges } merge committed but reported a maintenance failure after commit. See 'Committed merge maintenance failures' above. Run '{ $rescan_command }' to refresh the index.
       *[other] { $repaired } orphan facet(s) repaired; { $merges } merge(s) committed but reported a maintenance failure after commit. See 'Committed merge maintenance failures' above. Run '{ $rescan_command }' to refresh the index.
        }
    }

journal.cli.journal.facet.doctor.retired.malformed = { $retired_path } does not parse ({ $detail }). Run with { $fix_flag } to rebuild it; no name it reserves is freed.

journal.cli.journal.facet.doctor.retired.rebuilt = { $retired_path } did not parse ({ $detail }); it was moved to { $path } and rebuilt.

journal.cli.journal.facet.doctor.retired.reserved = These names could not be traced and are recorded as deleted: { $names }.

journal.cli.journal.facet.doctor.retired.unreadable = { $retired_path } could not be read ({ $detail }). Check its permissions; it was left as it is.

journal.cli.journal.facet.doctor.search.failed = search update could not be queued: { $error }

journal.cli.journal.facet.doctor.search.queued = search is catching up with the facet changes.

journal.cli.journal.facet.doctor.search.skipped = Search was not updated, because { $retired_path } can't be used yet.

journal.cli.journal.facet.doctor.twins.heading = Folders named like a facet you have:

journal.cli.journal.facet.doctor.twins.merge = These are never registered beside it. Run with { $adopt_flag } { $merge_flag } to fold them into it; a merge is permanent, and a file both have keeps the facet's copy.

journal.cli.journal.facet.doctor.twins.preview = To see what one would bring first: { $merge_command } { $source } { $into_flag } { $destination } { $dry_run_flag }

journal.cli.journal.facet.doctor.unclear.heading = Folders left as they are

journal.cli.journal.facet.doctor.variants.heading = Name-variant groups:

journal.cli.journal.facet.doctor.variants.merge = These were not registered. Run with { $adopt_flag } { $merge_flag } to collapse them into one.

journal.cli.journal.facet.doctor.variants.register = Run with { $adopt_flag } { $merge_flag } to collapse name variants before registering them.

journal.cli.journal.facet.entry.unsafe = unsafe facet entry: { $path }

journal.cli.journal.facet.hidden.damaged = the hidden names at { $path } can't be read, so nothing was changed: { $detail }

journal.cli.journal.facet.hidden.empty = no name was given

journal.cli.journal.facet.hidden.field.text = { $key } is not text

journal.cli.journal.facet.hidden.field.unknown = unknown field { $key }

journal.cli.journal.facet.hidden.names.list = { $names_field } is not a list

journal.cli.journal.facet.hidden.row.empty = name is empty

journal.cli.journal.facet.hidden.row.error = name { $index }: { $detail }

journal.cli.journal.facet.hidden.row.object = not an object

journal.cli.journal.facet.json.object = { $path } is not a { $json_format } object

journal.cli.journal.facet.link.conflict = { $path } has different contents in the folders being combined, so nothing was combined

journal.cli.journal.facet.link.file.encoding = { $path } is not { $encoding } text

journal.cli.journal.facet.link.ids.exhausted = notes have run out of id numbers

journal.cli.journal.facet.link.invalid.name = '{ $name }' can't be used as an entity id

journal.cli.journal.facet.link.not.file = { $path } is a folder or link, not a file, so nothing was combined

journal.cli.journal.facet.link.observation.corrupt = corrupt observation in { $source }: { $detail }

journal.cli.journal.facet.link.observation.malformed = malformed observation in { $source } at line { $line }: { $reason }

journal.cli.journal.facet.link.occupied = folder '{ $id }' holds another entity or can't be read

journal.cli.journal.facet.link.repair.destination = the folder for '{ $id }' in the facet being merged into holds another entity or can't be read, so nothing was changed; run '{ $doctor_command }' to see what needs repair

journal.cli.journal.facet.link.repair.named = the folder for '{ $id }' in the '{ $facet }' facet holds another entity or can't be read, so nothing was changed; run '{ $doctor_command }' to see what needs repair

journal.cli.journal.facet.link.successor.missing = the entity it joined can't be found

journal.cli.journal.facet.log.action.queue = action log search update could not be queued: { $error }

journal.cli.journal.facet.log.detections.queue = facet detections not queued for search facet={ $facet } cause={ $error }

journal.cli.journal.facet.log.trash.unfinished = trash item { $id }: moved, finishing later: { $error }

journal.cli.journal.facet.merge.argument.confirm = a merge is permanent and can't be undone; review it with { $dry_run_flag }, then run it again with { $yes_flag }

journal.cli.journal.facet.merge.argument.dest = { $into_flag } requires { $destination_argument }

journal.cli.journal.facet.merge.argument.into = { $into_dest } is required

journal.cli.journal.facet.merge.argument.invalid = invalid { $source_argument } or { $destination_argument }

journal.cli.journal.facet.merge.argument.source = { $source_argument } is required

journal.cli.journal.facet.merge.cleanup.failed = the merge finished, but its leftover folders couldn't be removed: { $error }; run '{ $repair_command }' to tidy them

journal.cli.journal.facet.merge.collisions.kept = Both facets had these files. '{ $destination }' kept its own copy; the copy in '{ $source }' was kept, and '{ $repair_command }' moves it to the trash:

journal.cli.journal.facet.merge.collisions.trashed = Both facets had these files. '{ $destination }' kept its own copy; the copy in '{ $source }' is in the trash:

journal.cli.journal.facet.merge.complete = Merged '{ $source }' into '{ $destination }'. Index rebuild completed.

journal.cli.journal.facet.merge.destination.absent = '{ $destination }' isn't a facet

journal.cli.journal.facet.merge.destination.id = '{ $destination }' has no stable id yet; run '{ $backfill_command }' first

journal.cli.journal.facet.merge.destination.retired = '{ $destination }' is recorded as a retired facet name; run '{ $doctor_command }' for details

journal.cli.journal.facet.merge.detections.failed = its detections were not queued

journal.cli.journal.facet.merge.failed = facet merge failed

journal.cli.journal.facet.merge.help = Usage: { $syntax }

journal.cli.journal.facet.merge.preview.empty = Nothing.

journal.cli.journal.facet.merge.preview.entities = { $count ->
    [one] { $count } entity is linked in both facets; it would end up with one link, keeping every note.
   *[other] { $count } entities are linked in both facets; each would end up with one link, keeping every note.
    }

journal.cli.journal.facet.merge.preview.fields = Entity fields where '{ $destination }' would keep its own, different value:

journal.cli.journal.facet.merge.preview.files = Files both facets have that can't be combined. '{ $destination }' would keep its own copy, and the copy in '{ $source }' would go to the trash:

journal.cli.journal.facet.merge.preview.heading = Dry run, nothing was changed. Merging '{ $source }' into '{ $destination }' would lose:

journal.cli.journal.facet.merge.preview.notes = { $count ->
    [one] { $count } note would get a new number, after the notes already there.
   *[other] { $count } notes would get a new number, after the notes already there.
    }

journal.cli.journal.facet.merge.preview.records = Records that would be dropped because a different record with the same id is kept (the first record with each id is kept, reading '{ $destination }' first):

journal.cli.journal.facet.merge.preview.settings = The settings in the { $declaration } of '{ $source }', which would not be carried over.

journal.cli.journal.facet.merge.queue.failed = the repair finished, but its search update could not be queued: { $error }

journal.cli.journal.facet.merge.scratch.failed = dry run could not remove its scratch copy { $path }: { $error }; nothing else was changed, and that folder can be deleted

journal.cli.journal.facet.merge.source.absent = '{ $source }' isn't a facet

journal.cli.journal.facet.merge.source.declared = '{ $source }' is a declared facet, not an orphan folder

journal.cli.journal.facet.merge.source.only = '{ $source }' is the only facet that isn't muted, and a journal keeps at least one; unmute '{ $destination }' first

journal.cli.journal.facet.merge.source.retired = '{ $source }' is a retired facet name

journal.cli.journal.facet.merge.trash.failed = the merge finished, but the files it replaced couldn't be moved to the trash: { $error }; they're kept, and '{ $repair_command }' moves them there

journal.cli.journal.facet.name.encoding = facet name is not { $encoding }

journal.cli.journal.facet.name.unsafe = unsafe facet declaration for { $slug }

journal.cli.journal.facet.notify.queue = the search update could not be queued: { $error }

journal.cli.journal.facet.notify.search = search was not updated: { $reason }

journal.cli.journal.facet.path.encoding = { $path } isn't { $encoding }

journal.cli.journal.facet.path.plain = { $path } isn't a plain path

journal.cli.journal.facet.record.declaration.damaged = facet declaration is damaged: { $detail }

journal.cli.journal.facet.record.declaration.exists = facet declaration already exists: { $path }

journal.cli.journal.facet.record.declaration.invalid = facet declaration is not an object: { $path }

journal.cli.journal.facet.record.declaration.missing = facet declaration is missing: { $path }

journal.cli.journal.facet.record.declaration.unreadable = facet declaration could not be read: { $detail }

journal.cli.journal.facet.record.id.duplicate = duplicate well-formed facet identifier already on disk: { $id }

journal.cli.journal.facet.record.id.entropy = could not obtain random bytes for identifier: { $error }

journal.cli.journal.facet.record.id.exhausted = exhausted retries generating a unique facet identifier

journal.cli.journal.facet.record.link.invalid = facet entity link is not an object: { $path }

journal.cli.journal.facet.record.malformed.activity = cannot read activity definitions at { $path } line { $line }: { $reason }

journal.cli.journal.facet.record.malformed.observation = malformed observation { $source } line { $line }: { $reason }

journal.cli.journal.facet.record.marker.invalid = facet entity-link repair completion marker is empty or malformed: { $path }

journal.cli.journal.facet.record.minimum = facet '{ $facet }' is the only enabled facet; a journal keeps at least one

journal.cli.journal.facet.record.missing.names = missing { $names_field } object

journal.cli.journal.facet.record.name.retired = the facet name '{ $name }' belonged to a facet that was merged or renamed, and leads to it now

journal.cli.journal.facet.record.not.object = not a { $json_format } object

journal.cli.journal.facet.record.reason.content = expected an object with nonblank string content

journal.cli.journal.facet.record.reason.history = invalid { $history_field } field

journal.cli.journal.facet.record.reason.id = { $id_field } must be an unsigned integer

journal.cli.journal.facet.record.reason.json = invalid { $json_format }

journal.cli.journal.facet.record.reason.object = expected an object

journal.cli.journal.facet.record.reason.relation = expected an object with nonblank string content or relation

journal.cli.journal.facet.record.reason.retired = invalid { $retired_field } field

journal.cli.journal.facet.record.retired.conflict = the facet name '{ $name }' is already recorded as retired with a different outcome

journal.cli.journal.facet.record.retired.damaged = { $retired_path } could not be read ({ $detail }); run '{ $repair_command }'

journal.cli.journal.facet.record.source.snapshot = captured snapshot

journal.cli.journal.facet.retire.argument = { $retire_flag } needs { $name_argument } and then { $into_facet } or { $deleted_flag }

journal.cli.journal.facet.retire.deleted = Recorded '{ $name }' as deleted. search is catching up.

journal.cli.journal.facet.retire.encoding = names must be { $encoding }

journal.cli.journal.facet.retire.folder = '{ $name }' is a folder in this journal; only a name that no longer exists can be retired

journal.cli.journal.facet.retire.merged = Recorded '{ $name }' as merged into '{ $target }'. search is catching up.

journal.cli.journal.facet.retire.name = '{ $name }' isn't a facet name

journal.cli.journal.facet.retire.target = '{ $target }' isn't a facet with a stable id; run '{ $backfill_command }' first

journal.cli.journal.facet.transaction.exists = transaction path already exists: { $path }

journal.cli.journal.facet.transaction.rollback = transaction failed and was rolled back: { $primary }

journal.cli.journal.facet.transaction.rollback.failed = transaction failed ({ $primary }); rollback also failed ({ $rollback_error })

journal.cli.journal.facet.trash.absent = that is no longer in the trash

journal.cli.journal.facet.trash.busy = that is already being recovered or erased

journal.cli.journal.facet.trash.io = your trash couldn't be changed: { $error }

journal.cli.journal.facet.trash.missing = there is no such { $item } in your journal

journal.cli.journal.facet.trash.unfinished = it's in the trash, which will finish putting it away

journal.cli.journal.facet.trash.unknown = this build can't recover that

journal.cli.journal.help.overview = { $invocation } - the journal on this computer{ $line_break }{ $line_break }Usage: { $syntax }{ $line_break }{ $line_break }{ $old_command } has moved to '{ $invocation }'; use that command for local journal operations.{ $line_break }{ $line_break }Local commands:{ $line_break }{ $commands }{ $line_break }{ $line_break }Process commands:{ $line_break }{ $processes }{ $line_break }{ $line_break }Journal-local commands:{ $line_break }{ $local }{ $line_break }{ $line_break }Options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_4 }Show this help{ $line_break }{ $syntax_continuation_17 }{ $line_break }{ $syntax_continuation_18 }

journal.cli.journal.help.path = usage: { $syntax }{ $line_break }{ $line_break }Print the resolved journal path.

journal.cli.journal.help.root = usage: { $syntax }{ $line_break }{ $line_break }Print the project root.

journal.cli.journal.help.setup.copy = usage: { $syntax }

journal.cli.journal.help.status = usage: { $syntax }{ $line_break }{ $line_break }Print whether the journal exists and how many days it has.

journal.cli.journal.help.usage = Usage: { $syntax }

journal.cli.journal.helper.inspect = { $code }: could not inspect the native journal executable: { $error }

journal.cli.journal.helper.missing.mac = { $code }: { $path }. Reinstall the journal app.

journal.cli.journal.helper.missing.other = { $code }: { $path }. Reinstall { $package }.

journal.cli.journal.helper.mode.mac = { $code }: { $path }. Reinstall the journal app.

journal.cli.journal.helper.mode.other = { $code }: { $path }. Reinstall { $package }.

journal.cli.journal.host.path.utf8 = native journal path resolution failed: resolved journal path is not valid UTF-8

journal.cli.journal.host.status.absent = Journal: { $path }{ $line_break }Source: { $source }{ $line_break }Exists: no{ $line_break }Days: { $days }

journal.cli.journal.host.status.directory = native journal status failed: journal root is not a directory: { $path }

journal.cli.journal.host.status.exists = Journal: { $path }{ $line_break }Source: { $source }{ $line_break }Exists: yes{ $line_break }Days: { $days }

journal.cli.journal.host.status.inspect = native journal status failed: could not inspect { $path }: { $error }

journal.cli.journal.host.status.utf8 = native journal status failed: resolved journal path is not valid UTF-8

journal.cli.journal.indexer.argument.argument = unexpected argument

journal.cli.journal.indexer.argument.budget = { $budget_seconds_flag } needs a positive number

journal.cli.journal.indexer.argument.drain = { $drain_flag } requires { $apply_flag }

journal.cli.journal.indexer.argument.duplicate = unexpected or duplicate argument

journal.cli.journal.indexer.argument.file = { $rescan_file_flag } requires { $path }

journal.cli.journal.indexer.argument.ios = unavailable on { $ios }

journal.cli.journal.indexer.argument.missing.agent = { $agent_flag } requires { $agent }

journal.cli.journal.indexer.argument.missing.day = { $day_flag } requires { $day }

journal.cli.journal.indexer.argument.missing.day.from = { $day_from_flag } requires { $day }

journal.cli.journal.indexer.argument.missing.day.to = { $day_to_flag } requires { $day }

journal.cli.journal.indexer.argument.missing.facet = { $facet_flag } requires { $facet }

journal.cli.journal.indexer.argument.missing.limit = { $limit_flag } requires a non-negative integer

journal.cli.journal.indexer.argument.missing.offset = { $offset_flag } requires a non-negative integer

journal.cli.journal.indexer.argument.missing.stream = { $stream_flag } requires { $stream }

journal.cli.journal.indexer.argument.missing.top = { $top_flag } requires a non-negative integer

journal.cli.journal.indexer.argument.rescan.conflict = { $rescan_file_flag } cannot be combined with { $rescan_flag } or { $rescan_full_flag }

journal.cli.journal.indexer.argument.unsupported = unsupported file

journal.cli.journal.indexer.build.building = build: building

journal.cli.journal.indexer.build.complete = build: complete

journal.cli.journal.indexer.build.unknown = build: unknown

journal.cli.journal.indexer.calendar.at = line { $line }: { $reason }

journal.cli.journal.indexer.calendar.import = calendar notes in { $rel } skipped: { $error }

journal.cli.journal.indexer.calendar.line.repeat = calendar note line { $line } repeats an event

journal.cli.journal.indexer.calendar.line.skip = calendar note line { $line } skipped: { $reason }

journal.cli.journal.indexer.calendar.segment = calendar notes skipped: { $rel } is not in a segment

journal.cli.journal.indexer.calendar.settings = calendar event in { $path } is listed once the settings file can be read: { $error }

journal.cli.journal.indexer.calendar.source = calendar notes are read only from calendar streams and calendar imports: { $rel }

journal.cli.journal.indexer.calendar.unlisted = calendar note in { $path } not listed: { $reason }

journal.cli.journal.indexer.calendar.validation.account.must.be.a.string.or.null = { $account_field } must be a string or { $null_key }

journal.cli.journal.indexer.calendar.validation.all.day.must.be.true.or.false = { $all_day_field } must be { $true_key } or { $false_key }

journal.cli.journal.indexer.calendar.validation.attendee.status.must.be.a.string.or.null = attendee { $status_field } must be a string or { $null_key }

journal.cli.journal.indexer.calendar.validation.attendee.you.must.be.true.or.false = attendee { $you_field } must be { $true_key } or { $false_key }

journal.cli.journal.indexer.calendar.validation.attendees.must.be.an.array = { $attendees_field } must be an array

journal.cli.journal.indexer.calendar.validation.change.must.be.added.changed.or.removed = { $change_field } must be { $added_changed_or_removed_key }

journal.cli.journal.indexer.calendar.validation.changed.at.falls.outside.the.segment.window = { $changed_at_field } falls outside the segment window

journal.cli.journal.indexer.calendar.validation.changed.at.must.be.a.time.with.an.offset = { $changed_at_field } must be a time with an offset

journal.cli.journal.indexer.calendar.validation.each.attendee.must.be.an.object = each attendee must be an object

journal.cli.journal.indexer.calendar.validation.end.cannot.be.read.as.a.date.or.time = { $end_field } cannot be read as a date or time

journal.cli.journal.indexer.calendar.validation.end.is.before.start = { $end_field } is before start

journal.cli.journal.indexer.calendar.validation.end.must.be.a.string.or.null = { $end_field } must be a string or { $null_key }

journal.cli.journal.indexer.calendar.validation.event.and.occurrence.repeat.an.earlier.line = { $event_field } and occurrence repeat an earlier line

journal.cli.journal.indexer.calendar.validation.event.must.be.a.non.empty.string = { $event_field } must be a non-empty string

journal.cli.journal.indexer.calendar.validation.expanded.must.be.true.when.present = { $expanded_field } must be { $true_key } when present

journal.cli.journal.indexer.calendar.validation.field.missing = { $key } is missing

journal.cli.journal.indexer.calendar.validation.field.type = { $key } must be a string or { $null_input }

journal.cli.journal.indexer.calendar.validation.file.encoding = file is not { $encoding }

journal.cli.journal.indexer.calendar.validation.file.holds.no.notes = file holds no notes

journal.cli.journal.indexer.calendar.validation.file.lines = file holds more than 5,000 notes

journal.cli.journal.indexer.calendar.validation.file.size = file holds more than 8 { $unit }

journal.cli.journal.indexer.calendar.validation.filename = file name must be { $prefix } followed by 64 lowercase hex digits and { $suffix }

journal.cli.journal.indexer.calendar.validation.import.must.be.a.non.empty.string = { $import_field } must be a non-empty string

journal.cli.journal.indexer.calendar.validation.imported.at.must.be.a.time.with.an.offset = { $imported_at_key } must be a time with an offset

journal.cli.journal.indexer.calendar.validation.line.is.empty = line is empty

journal.cli.journal.indexer.calendar.validation.line.is.not.a.json.object = line is not a { $json_key } object

journal.cli.journal.indexer.calendar.validation.occurrence.cannot.be.read.as.a.date.or.time = { $occurrence_field } cannot be read as a date or time

journal.cli.journal.indexer.calendar.validation.occurrence.must.be.a.string.or.null = { $occurrence_field } must be a string or { $null_key }

journal.cli.journal.indexer.calendar.validation.organizer.must.be.an.object.or.null = { $organizer_field } must be an object or { $null_key }

journal.cli.journal.indexer.calendar.validation.person.field = { $what } { $key } must be a string or { $null_input }

journal.cli.journal.indexer.calendar.validation.segment.grid = segment must name one 300-second window on the journal's grid

journal.cli.journal.indexer.calendar.validation.segment.must.name.a.span.on.its.day = segment must name a span on its day

journal.cli.journal.indexer.calendar.validation.start.and.end.must.be.dates.exactly.when.all.day.is.true = { $start_field } and { $end_field } must be dates exactly when { $all_day_field } is { $true_key }

journal.cli.journal.indexer.calendar.validation.start.and.end.must.both.carry.an.offset.or.both.have.none = { $start_field } and { $end_field } must both carry an offset or both have none

journal.cli.journal.indexer.calendar.validation.start.cannot.be.read.as.a.date.or.time = { $start_field } cannot be read as a date or time

journal.cli.journal.indexer.calendar.validation.start.is.missing = { $start_field } is missing

journal.cli.journal.indexer.calendar.validation.version = { $v_field } must be 1

journal.cli.journal.indexer.chunker.activity = unsupported desktop activity version

journal.cli.journal.indexer.chunker.calendar = calendar note skipped: { $reason }

journal.cli.journal.indexer.chunker.lines = { $collapsed ->
    [one] Collapsed whitespace on { $collapsed } overlong Markdown line during indexing
   *[other] Collapsed whitespace on { $collapsed } overlong Markdown line(s) during indexing
    }

journal.cli.journal.indexer.classification.absent = classifications: database absent

journal.cli.journal.indexer.classification.complete = classifications: complete (processed { $processed }, unclassified { $unclassified })

journal.cli.journal.indexer.classification.incomplete = classifications: incomplete sidecar at cursor '{ $cursor }' (run '{ $repair }')

journal.cli.journal.indexer.classification.mismatch = classifications: coverage mismatch at cursor '{ $cursor }' (processed { $processed }, remaining { $remaining }, missing { $missing }, unclassified { $unclassified })

journal.cli.journal.indexer.classification.missing = { $missing ->
    [one] chunk classification coverage mismatch: { $missing } missing row
   *[other] chunk classification coverage mismatch: { $missing } missing rows
    }

journal.cli.journal.indexer.classification.progress = classifications: in progress at cursor '{ $cursor }' (processed { $processed }, remaining { $remaining }, missing { $missing }, unclassified { $unclassified })

journal.cli.journal.indexer.classification.stall = chunk classification backfill stalled: { $err }

journal.cli.journal.indexer.classification.stalled = classifications: stalled at '{ $path }' (processed { $processed }, remaining { $remaining }, missing { $missing }, unclassified { $unclassified })

journal.cli.journal.indexer.classification.unready = classifications: unready at cursor '{ $cursor }' (run '{ $repair_command }')

journal.cli.journal.indexer.database.absent = index: absent

journal.cli.journal.indexer.database.files = files: { $discovered } on disk, { $indexed } indexed, { $current } current, { $stale } stale, { $missing } missing, { $failed } failed, { $orphaned } orphaned, { $retained } kept until a full rescan, { $ineligible_rows } no longer indexable but still in search, { $unreadable } unreadable, { $ineligible } not indexable

journal.cli.journal.indexer.database.present = index: present

journal.cli.journal.indexer.database.sqlite = not a { $sqlite } database

journal.cli.journal.indexer.database.unreadable = index: unreadable ({ $reason })

journal.cli.journal.indexer.discovery.listing = glob traversal failed: attempting to read `{ $path }` resulted in an error: { $error }

journal.cli.journal.indexer.discovery.pattern = invalid glob pattern: { $error }

journal.cli.journal.indexer.discovery.prefix = discovered path { $path } is not under root { $root }

journal.cli.journal.indexer.discovery.relative = discovered path is not valid { $encoding }: { $path }

journal.cli.journal.indexer.discovery.root = glob root is not valid { $encoding }: { $path }

journal.cli.journal.indexer.drain.result = { $processed ->
    [one] applied { $processed } search update; { $failed } failed; { $remaining } { $remaining ->
        [one] remains
       *[other] remain
    } queued.
   *[other] applied { $processed } search updates; { $failed } failed; { $remaining } { $remaining ->
        [one] remains
       *[other] remain
    } queued.
    }

journal.cli.journal.indexer.edge.blob = edge row contains an unsupported blob value

journal.cli.journal.indexer.edge.day = Invalid edge day: { $day }

journal.cli.journal.indexer.edge.dst = edge row requires non-empty string { $dst_field }

journal.cli.journal.indexer.edge.event = event timestamp is outside { $integer } range

journal.cli.journal.indexer.edge.integer = edge integer field { $field } is outside { $integer } range

journal.cli.journal.indexer.edge.json = edge source { $json } parse failed for { $path }: { $error }

journal.cli.journal.indexer.edge.kind = Unknown edge kind: { $kind }

journal.cli.journal.indexer.edge.path = invalid observations path: { $path }

journal.cli.journal.indexer.edge.payload = { $source } payload must be a { $json } object, got { $value_type }

journal.cli.journal.indexer.edge.refresh = Skipping edge refresh for { $rel }: { $error }

journal.cli.journal.indexer.edge.relation = observation relation is missing { $kind_field }

journal.cli.journal.indexer.edge.segment.day = Invalid segment day: { $day }

journal.cli.journal.indexer.edge.segment.key = Invalid segment key: { $segment }

journal.cli.journal.indexer.edge.segment.time = segment timestamp is outside { $integer } range

journal.cli.journal.indexer.edge.src = edge row requires non-empty string { $src_field }

journal.cli.journal.indexer.edge.string = cannot reproduce { $python } for { $field } value of type { $value_type }

journal.cli.journal.indexer.edge.target = observation relation { $target_field } must be a non-empty string

journal.cli.journal.indexer.edge.type = edge kind for { $source } must be a string, got { $value_type }

journal.cli.journal.indexer.edge.unsupported = edge field { $field } does not support { $value_type }

journal.cli.journal.indexer.error.file = rescan-file failed: { $detail }

journal.cli.journal.indexer.error.rebuild = edge rebuild failed: { $detail }

journal.cli.journal.indexer.error.reset = reset failed: { $detail }

journal.cli.journal.indexer.error.scan = scan failed: { $detail }

journal.cli.journal.indexer.files.activity.membership.insert = activity membership insert failed for { $rel }: { $error }

journal.cli.journal.indexer.files.activity.provenance.insert = activity provenance insert failed for { $rel }: { $error }

journal.cli.journal.indexer.files.calendar.listing = calendar listing failed for { $rel }: { $error }

journal.cli.journal.indexer.files.calendar.note.insert = calendar note insert failed for { $rel }: { $error }

journal.cli.journal.indexer.files.chunk.group.offset.insert = chunk group offset insert failed for { $rel }: { $error }

journal.cli.journal.indexer.files.chunk.insert = chunk insert failed for { $rel }: { $error }

journal.cli.journal.indexer.files.chunk.source.mapping = chunk source mapping failed for { $rel }: { $error }

journal.cli.journal.indexer.files.classification.insert = classification insert failed for { $rel }: { $error }

journal.cli.journal.indexer.files.content.read = content read failed for { $rel }: { $error }

journal.cli.journal.indexer.files.facet = facet declaration scan failed for { $rel }: { $error }

journal.cli.journal.indexer.files.hidden = { $rel } is left out of search for now: { $cause }

journal.cli.journal.indexer.files.memory.chain.serialization = memory chain serialization failed for { $rel }: { $error }

journal.cli.journal.indexer.files.memory.missing = validated memory original missing for { $rel }

journal.cli.journal.indexer.files.memory.origin.serialization = memory origin serialization failed for { $rel }: { $error }

journal.cli.journal.indexer.files.memory.original.insert = memory original insert failed for { $rel }: { $error }

journal.cli.journal.indexer.files.memory.utf = memory original is not { $utf } for { $rel }: { $error }

journal.cli.journal.indexer.files.partial = { $rel } is partly left out of search for now: { $cause }

journal.cli.journal.indexer.files.screen.text.source.insert = screen text source insert failed for { $rel }: { $error }

journal.cli.journal.indexer.generation.locked = generation: { $generation } (this version reads up to { $known }; writable: no)

journal.cli.journal.indexer.generation.writable = generation: { $generation } (this version reads up to { $known }; writable: yes)

journal.cli.journal.indexer.help.drain = Usage: { $syntax }{ $line_break }apply queued search updates. { $force_flag } includes delayed retries.{ $line_break }{ $budget_seconds_flag } stops the drain after N seconds. a whole-journal update does not start with under 300 seconds left and stays queued.

journal.cli.journal.indexer.help.main = Usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $syntax_continuation_3 }{ $line_break }{ $syntax_continuation_4 }

journal.cli.journal.indexer.help.path = Usage: { $syntax }

journal.cli.journal.indexer.help.status = Usage: { $syntax }

journal.cli.journal.indexer.hidden.activity = can't tell whether this activity is hidden: { $rel } can't be read: { $error }

journal.cli.journal.indexer.hidden.damaged = can't tell whether this activity is hidden: { $path } has a line that isn't a record

journal.cli.journal.indexer.hidden.days = can't tell which days of body data are hidden: { $path } can't be read

journal.cli.journal.indexer.hidden.trash = can't tell what the journal is making again: { $error }

journal.cli.journal.indexer.hidden.week = can't tell what is left out of this week page: { $path } can't be read

journal.cli.journal.indexer.hidden.weeks = can't tell what is left out of week pages: { $path } can't be read

journal.cli.journal.indexer.merge.commit = index is correct and merely uncompacted: could not commit { $fts } merge: { $error }

journal.cli.journal.indexer.merge.failed = index is correct and merely uncompacted: { $fts } merge failed: { $error }

journal.cli.journal.indexer.merge.start = index is correct and merely uncompacted: could not start { $fts } merge transaction: { $error }

journal.cli.journal.indexer.migration.child = segment aggregate migration retained { $rel_segment }: child file is not current: { $rel }

journal.cli.journal.indexer.migration.discovery = segment aggregate migration retained { $rel_segment }: talent discovery failed: { $error }

journal.cli.journal.indexer.migration.files = index files remain after migration: { $paths }

journal.cli.journal.indexer.migration.mtime = segment aggregate migration retained { $rel_segment }: mtime read failed for { $rel }: { $error }

journal.cli.journal.indexer.migration.retained = segment aggregate migration retained { $rel }: { $error }

journal.cli.journal.indexer.migration.unclassified = segment aggregate migration retained { $rel_segment }: unclassified talent Markdown { $rel }

journal.cli.journal.indexer.original.incomplete = memory original is incomplete for { $rel }: { $state }

journal.cli.journal.indexer.original.retained = memory original could not be checked for { $rel }; cached index retained: { $state }

journal.cli.journal.indexer.original.state.absent = absent

journal.cli.journal.indexer.original.state.corrupt = corrupt

journal.cli.journal.indexer.original.state.deleted = deleted

journal.cli.journal.indexer.original.state.ready = ready

journal.cli.journal.indexer.original.state.staged = staged

journal.cli.journal.indexer.original.state.unavailable = unavailable ({ $detail })

journal.cli.journal.indexer.original.state.unready = unready

journal.cli.journal.indexer.path.ready = path lookup: ready

journal.cli.journal.indexer.path.unready = path lookup: unready

journal.cli.journal.indexer.progress.absent = classification: absent

journal.cli.journal.indexer.progress.complete = classification: complete

journal.cli.journal.indexer.progress.incomplete = classification: incomplete

journal.cli.journal.indexer.progress.stalled = classification: stalled at { $path }

journal.cli.journal.indexer.progress.unknown = classification: stalled at unknown path

journal.cli.journal.indexer.query.absent = journal index is absent: { $path }

journal.cli.journal.indexer.query.column.agent = Agent

journal.cli.journal.indexer.query.column.day = Day

journal.cli.journal.indexer.query.column.facet = Facet

journal.cli.journal.indexer.query.empty = journal index is empty: { $path }

journal.cli.journal.indexer.query.error = { $command }: { $detail }

journal.cli.journal.indexer.query.input = stdin read failed: { $error }

journal.cli.journal.indexer.query.locked = journal index is locked ({ $path }): { $detail }

journal.cli.journal.indexer.query.more = ... +{ $remaining } more

journal.cli.journal.indexer.query.none = No results found

journal.cli.journal.indexer.query.output = stdout write failed: { $error }

journal.cli.journal.indexer.query.owner = connection query needs the owner

journal.cli.journal.indexer.query.prompt = search>

journal.cli.journal.indexer.query.showing = { $total ->
    [one] Showing { $first }-{ $last } of { $total } result
   *[other] Showing { $first }-{ $last } of { $total } results
    }

journal.cli.journal.indexer.query.total = { $total ->
    [one] Total: { $total } chunk
   *[other] Total: { $total } chunks
    }

journal.cli.journal.indexer.query.unreadable = journal index is unreadable ({ $path }): { $detail }

journal.cli.journal.indexer.receipt.absent = last scan: not recorded

journal.cli.journal.indexer.receipt.full.finished = last scan: full at { $time } ms, indexed { $indexed }, removed { $removed }, failed { $failed }

journal.cli.journal.indexer.receipt.full.stopped = last scan: full at { $time } ms stopped: { $error }

journal.cli.journal.indexer.receipt.light.finished = last scan: light at { $time } ms, indexed { $indexed }, removed { $removed }, failed { $failed }

journal.cli.journal.indexer.receipt.light.stopped = last scan: light at { $time } ms stopped: { $error }

journal.cli.journal.indexer.rel.absolute = { $rel } must be journal-{ $rel }ative

journal.cli.journal.indexer.rel.backslash = { $rel } must use { $posix } separators

journal.cli.journal.indexer.rel.empty = { $rel } must be non-empty

journal.cli.journal.indexer.rel.invalidcomponent = { $rel } must not contain empty, { $current_component }, or { $parent_component } components

journal.cli.journal.indexer.rel.trash = { $rel } must not name the trash

journal.cli.journal.indexer.scan.calendar.invites = calendar invites not linked yet: { $error }

journal.cli.journal.indexer.scan.connections = No new connections were added in this rescan. Run { $rebuild_command } to bring any still waiting up to date now.

journal.cli.journal.indexer.scan.connections.failed = connections not checked against entity changes: { $error }

journal.cli.journal.indexer.scan.deferred = a full search pass is already working through these changes, so this check left them to it. run { $status_command } for details.

journal.cli.journal.indexer.scan.edge.skip = Skipping edge extraction for { $rel }: { $error }

journal.cli.journal.indexer.scan.failed = { $indexed ->
    [one] Indexed { $indexed } file, removed { $removed }, skipped { $skipped }, failed { $failed }
   *[other] Indexed { $indexed } file(s), removed { $removed }, skipped { $skipped }, failed { $failed }
    }

journal.cli.journal.indexer.scan.failure = { $failed ->
    [one] { $command }: { $failed } update couldn't be indexed; see the warnings above
   *[other] { $command }: { $failed } update(s) couldn't be indexed; see the warnings above
    }

journal.cli.journal.indexer.scan.invalid.segment = Skipping edge extraction for { $rel }: invalid segment key { $segment }

journal.cli.journal.indexer.scan.mtime = mtime read failed for { $rel }: { $error }

journal.cli.journal.indexer.scan.retained = { $count ->
    [one] light scan retained { $count } stale row for day { $day }: discovery produced no files for that day; rerun with { $full_flag } to remove it
   *[other] light scan retained { $count } stale row(s) for day { $day }: discovery produced no files for that day; rerun with { $full_flag } to remove them
    }

journal.cli.journal.indexer.scan.summary = { $indexed ->
    [one] Indexed { $indexed } file, removed { $removed }, skipped { $skipped }
   *[other] Indexed { $indexed } file(s), removed { $removed }, skipped { $skipped }
    }

journal.cli.journal.indexer.speaker.duplicate = speaker edge transcript duplicate { $field } { $sentence } for { $composite_id }

journal.cli.journal.indexer.speaker.missing = speaker edge transcript missing for { $composite_id }

journal.cli.journal.indexer.speaker.unresolved = speaker edge transcript unresolved for { $composite_id }

journal.cli.journal.indexer.status.check = last search check: at { $time } ms, indexed { $indexed }, removed { $removed }

journal.cli.journal.indexer.status.check.absent = last search check: not recorded

journal.cli.journal.indexer.status.check.overdue = last search check: overdue, it has run since then without finishing

journal.cli.journal.indexer.status.failure = failure: { $reason }

journal.cli.journal.indexer.status.health = health: { $state } ({ $description })

journal.cli.journal.indexer.status.queued = queued: { $queued }

journal.cli.journal.indexer.status.removals = removals still waiting: { $removals }

journal.cli.journal.indexer.status.wait.days = oldest queued: waiting { $days } d { $hours } h

journal.cli.journal.indexer.status.wait.hours = oldest queued: waiting { $hours } h { $minutes } min

journal.cli.journal.indexer.status.wait.minutes = oldest queued: waiting { $minutes } min

journal.cli.journal.indexer.status.wait.seconds = oldest queued: waiting { $seconds } s

journal.cli.journal.indexer.store.admission = index writer admission failed for { $operation }: { $cause }

journal.cli.journal.indexer.store.busy = index writer busy for { $operation }: { $cause }

journal.cli.journal.indexer.store.encoding = path is not valid { $encoding }: { $path }

journal.cli.journal.indexer.store.generation = the search index was written by a newer version of { $brand } (index generation { $found }; this version reads up to { $known }), so this version leaves it unchanged

journal.cli.journal.indexer.store.lookup.cause = { $lookup_command }: { $cause }

journal.cli.journal.indexer.store.missing = file not found: { $path }

journal.cli.journal.indexer.store.original = memory original could not be checked at { $path }; cached index retained; retry when the source is readable: { $detail }

journal.cli.journal.indexer.store.outside = file is outside journal directory: { $path }

journal.cli.journal.indexer.stream.json = stream marker { $syntax } failed for { $path }: { $error }

journal.cli.journal.indexer.stream.path = stream marker path failed for { $rel_segment }: { $error }

journal.cli.journal.indexer.stream.read = stream marker read failed for { $path }: { $error }

journal.cli.journal.indexer.warning = warning: { $warning }

journal.cli.journal.local.directory = not a real directory: { $path }

journal.cli.journal.local.failure = { $invocation } { $token }: { $message }

journal.cli.journal.local.resolution.config = journal resolution failed: config decode failed

journal.cli.journal.local.resolution.home = journal resolution failed: home unavailable

journal.cli.journal.local.unknown = unknown local operation

journal.cli.journal.notify.error.facet = { $invocation }: error: { $option } is no longer supported. Put facet selection in { $action_flag }; for example, { $action_flag } { $example }.

journal.cli.journal.notify.error.integer = { $invocation }: error: argument { $auto_dismiss_flag }: invalid int value: '{ $value }'

journal.cli.journal.notify.error.message = { $invocation }: error: the following arguments are required: { $positional }

journal.cli.journal.notify.error.unknown = { $invocation }: error: unrecognized arguments: { $token }

journal.cli.journal.notify.error.value = { $invocation }: error: argument { $option }: expected one argument

journal.cli.journal.notify.failed = Failed to send notification (is { $channel } running?)

journal.cli.journal.notify.help = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }                  { $message_options }{ $line_break }{ $line_break }Send a notification via { $channel }{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $positional }notification message text{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }show this help message and exit{ $line_break }{ $indent_2 }{ $title_option }{ $gap_9 }notification title{ $line_break }{ $indent_2 }{ $icon_option }{ $gap_11 }{ $icon_set } icon name (default: { $mailbox }){ $line_break }{ $indent_2 }{ $event_option }{ $gap_9 }event name (default: { $default_show }{ $line_break }{ $indent_2 }{ $action_option }{ $gap_7 }URL path to open on click{ $line_break }{ $indent_2 }{ $app_option }{ $gap_13 }source app name{ $line_break }{ $indent_2 }{ $badge_option }{ $gap_9 }badge text or number{ $line_break }{ $syntax_continuation_18 }{ $line_break }                        auto-dismiss after N milliseconds{ $line_break }{ $indent_2 }{ $no_dismiss_option }{ $gap_10 }make notification non-dismissible{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_9 }Enable verbose output{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_11 }Enable debug logging

journal.cli.journal.notify.sent = Notification sent

journal.cli.journal.notify.utf8 = native journal notify failed: owner arguments are not valid UTF-8

journal.cli.journal.process.admission = journal process admission failed: { $detail }

journal.cli.journal.process.body = native journal process launch failed: no native body for { $token }

journal.cli.journal.process.launch = native journal process launch failed: { $error }

journal.cli.journal.resolution.config = native journal resolution failed: config decode failed

journal.cli.journal.resolution.home = native journal resolution failed: home unavailable

journal.cli.journal.root.inspect = native journal project root resolution failed: could not inspect current executable: { $error }

journal.cli.journal.root.package = native journal project root resolution failed: could not locate source checkout or installed { $product } package from { $path }

journal.cli.share.activities = could not read { $facet } activities for { $day }: { $detail }

journal.cli.share.activity.missing = no activity '{ $activity }' in { $facet } on { $day }

journal.cli.share.activity.unmatched = activity '{ $activity }' does not match { $person }

journal.cli.share.facets = could not list facets: { $detail }

journal.cli.share.failure = { $command }: { $detail }

journal.cli.share.help.match = usage: { $syntax }{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $person_option }{ $gap_3 }entity id, or a unique name or alias of a person{ $line_break }{ $indent_2 }{ $day_option }{ $gap_4 }the day to read{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_5 }only this facet (default: every facet)

journal.cli.share.help.package = usage: { $syntax }{ $line_break }{ $line_break }options:{ $line_break }{ $indent_2 }{ $person_option }{ $gap_3 }entity id, or a unique name or alias of a person{ $line_break }{ $indent_2 }{ $day_option }{ $gap_4 }the activity's day{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_5 }the activity's facet{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_5 }the activity id

journal.cli.share.help.root = usage: { $syntax }{ $line_break }{ $line_break }Find activities shared with a linked person and preview what would be shared.{ $line_break }Both subcommands only read the journal.{ $line_break }{ $line_break }commands:{ $line_break }{ $match_command }List the day's activities that match a person, one { $json } line each{ $line_break }{ $package_command }Print the scoped summary package for one matching activity

journal.cli.share.parse.argument = { $command }: error: unrecognized argument '{ $argument }'

journal.cli.share.parse.duplicate = { $command }: error: { $flag } given more than once

journal.cli.share.parse.encoding = { $command }: error: arguments must be valid { $encoding }

journal.cli.share.parse.missing = { $command }: error: a subcommand is required

journal.cli.share.parse.required = { $command }: error: { $option } is required

journal.cli.share.parse.unknown = { $command }: error: unknown subcommand '{ $argument }'

journal.cli.share.parse.value = { $command }: error: { $flag } needs a value

journal.cli.share.provider.activity.unavailable = activity summary unavailable: { $detail }

journal.cli.share.provider.child.cleanup = { $primary } (cleanup: { $cleanup })

journal.cli.share.provider.child.exit = exit { $code }

journal.cli.share.provider.child.exit.signal = exit { $code } signal { $signal }

journal.cli.share.provider.child.invalid = invalid child response ({ $status }): { $detail }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.process = process I/O failed: { $primary }; { $status_field }{ $status }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.process.cleanup = process I/O failed: { $primary }; { $status_field }{ $status }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }; { $cleanup_field }{ $cleanup }

journal.cli.share.provider.child.protocol = protocol { $reason } ({ $status }): { $detail }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.signal = signal { $signal }

journal.cli.share.provider.child.status.unknown = status unknown

journal.cli.share.provider.child.truncated = { $bytes } [truncated]

journal.cli.share.provider.child.unavailable = unavailable

journal.cli.share.provider.child.unexpected = unexpected child ({ $status }); { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.entities.read = could not read entities: { $detail }

journal.cli.share.provider.entity.invalid = failed to load journal entity { $entity }: { $detail }

journal.cli.share.provider.matching = matching { $facet }/{ $day }/{ $activity } for { $entity }

journal.cli.share.provider.model.unconfigured = No model is chosen yet. Choose one in Thinking.

journal.cli.share.provider.person.absent = no person in this journal is named '{ $query }'

journal.cli.share.provider.person.ambiguous = '{ $query }' names more than one person ({ $people }); use an entity id

journal.cli.share.provider.presence.unavailable = presence check unavailable: { $detail }

journal.cli.share.provider.segment.unavailable = segment summary unavailable: { $detail }

journal.cli.share.provider.talent.invalid = talent '{ $talent }' returned invalid { $json }: { $error }

journal.cli.share.provider.talent.prepare = talent '{ $talent }': { $detail }

journal.cli.share.provider.talent.timeout.numeric = configuration '{ $talent }' declares a nonnumeric { $timeout_s_field }

journal.cli.share.provider.talent.timeout.range = configuration '{ $talent }' declares { $timeout_s_field } outside the supported range

journal.cli.share.provider.talent.tokens = talent '{ $talent }' declares no { $max_output_tokens_field }

journal.cli.share.provider.talent.type = unsupported talent type: { $kind }

journal.cli.share.provider.talent.unfinished = talent '{ $talent }' did not finish: { $outcome }

journal.cli.share.startup.failed = { $command } failed: { $detail }

journal.cli.share.startup.runtime.current = could not resolve worker executable: { $error }

journal.cli.share.startup.runtime.parent = worker executable has no parent: { $path }

journal.cli.share.startup.runtime.root = could not resolve installation root from executable directory: { $path }

journal.cli.share.startup.sibling.missing = { $command } failed: missing sibling executable { $path }

journal.cli.share.startup.sibling.parent = { $command } failed: current executable has no parent

journal.cli.share.transport.child.inspect = inspect child before termination: { $detail }

journal.cli.share.transport.child.reap = reap child: { $detail }

journal.cli.share.transport.child.retry = retry reap child: { $detail }

journal.cli.share.transport.child.terminate = terminate child: { $detail }

journal.cli.share.transport.collector = { $pipe } collector: { $detail }

journal.cli.share.transport.collector.panic = { $pipe } collector: thread panicked

journal.cli.share.transport.field.array = { $field } must be an array or null

journal.cli.share.transport.field.boolean = { $field } must be a boolean

journal.cli.share.transport.field.optional = { $field } must be a string or null

journal.cli.share.transport.field.string = { $field } must be a string

journal.cli.share.transport.field.strings = { $field } must contain only strings

journal.cli.share.transport.object = record must be a { $json } object

journal.cli.share.transport.outcome = response outcome is not supported

journal.cli.share.transport.pipe.missing = wire { $pipe } is unavailable

journal.cli.share.transport.request.empty = { $contents } must be non-empty

journal.cli.share.transport.request.overflow = { $overflow } requires the first content part to be text

journal.cli.share.transport.schema = record schema is not supported

journal.cli.share.transport.stdout.limit = { $stdout } truncated at { $limit } bytes

journal.cli.share.transport.usage = { $usage } is required

journal.cli.share.transport.worker.channel = pipe worker completion channel: { $detail }

journal.cli.share.transport.worker.join = { $pipe } worker join: thread panicked

journal.cli.share.transport.worker.start = start { $pipe } worker: { $detail }

journal.cli.share.transport.writer = { $stdin } writer: { $detail }

journal.cli.share.transport.writer.panic = { $stdin } writer: thread panicked

journal.cli.share.usage.match = usage: { $syntax }

journal.cli.share.usage.package = usage: { $syntax }

journal.cli.share.usage.root = usage: { $syntax }

journal.cli.doctor.device.day.listing.chronicle.error = device streams unavailable: chronicle unreadable: { $error }

journal.cli.doctor.device.day.listing.continuity.error = device streams unavailable: stream continuity unreadable: { $error }

journal.cli.doctor.device.day.listing.continuity.malformed = device streams unavailable: stream continuity unreadable: continuity record is malformed or unsupported

journal.cli.doctor.device.day.listing.record.line = device streams unavailable: stream registry unreadable: malformed stream record { $record }: malformed data in { $path } at line { $line }

journal.cli.doctor.device.day.listing.record.malformed = device streams unavailable: stream registry unreadable: malformed stream record { $record }: malformed data in { $path }

journal.cli.doctor.device.day.listing.registry.error = device streams unavailable: stream registry unreadable: { $error }

journal.cli.talent.daily.accepted.response = missing accepted response

journal.cli.talent.daily.accepted.result = missing accepted daily result

journal.cli.talent.daily.artifact.before = conflict: required artifact changed after prompt preparation

journal.cli.talent.daily.artifact.changed = conflict: required output changed after preparation

journal.cli.talent.daily.artifact.snapshot = missing frozen required-artifact snapshot

journal.cli.talent.daily.artifact.snapshot.invalid = invalid frozen artifact snapshot: { $error }

journal.cli.talent.daily.attempt.day = missing daily attempt day

journal.cli.talent.daily.attempt.day.invalid = invalid daily attempt day

journal.cli.talent.daily.attempt.mismatch = daily { $key } mismatch

journal.cli.talent.daily.attempt.token = missing daily attempt token

journal.cli.talent.daily.commit.observer = expected entity facts state

journal.cli.talent.daily.commit.suggest = expected entity suggest state

journal.cli.talent.daily.commit.text = expected text output

journal.cli.talent.daily.facet.changed = conflict: owning facet changed after prompt preparation

journal.cli.talent.daily.facet.identity = missing frozen facet identity

journal.cli.talent.daily.facet.path = invalid facet output path

journal.cli.talent.daily.maintenance.changed = conflict: daily time changed after prompt preparation

journal.cli.talent.daily.maintenance.output = invalid maintenance output: { $error }

journal.cli.talent.daily.maintenance.primary = maintenance output lacks primary

journal.cli.talent.daily.maintenance.snapshot = missing frozen daily-time snapshot

journal.cli.talent.daily.observer.changed = conflict: observation changed after prompt preparation

journal.cli.talent.daily.observer.conflict = conflict: { $error }

journal.cli.talent.daily.observer.entity.gone = conflict: observation entity disappeared

journal.cli.talent.daily.observer.validation = validation: { $error }

journal.cli.talent.daily.output.outside = daily output is outside journal

journal.cli.talent.daily.output.path = invalid daily output path

journal.cli.talent.daily.packet.absent = daily attempt has no frozen packet

journal.cli.talent.daily.packet.digest = daily packet digest mismatch

journal.cli.talent.daily.packet.scope = daily packet scope mismatch

journal.cli.talent.daily.packet.talent = daily packet talent mismatch

journal.cli.talent.daily.parse.entities = entities must be an array

journal.cli.talent.daily.parse.output = could not parse output as JSON: { $error }

journal.cli.talent.daily.publication.plan = publication requires the retained generated result and exact prepared plan

journal.cli.talent.daily.publication.record = missing publication record

journal.cli.talent.daily.response.malformed = retained daily response is malformed

journal.cli.talent.daily.retry.scope = daily retry scope changed

journal.cli.talent.daily.review.aliases = review promotion lacks aliases

journal.cli.talent.daily.review.eligibility = missing frozen review eligibility

journal.cli.talent.daily.review.hints = missing frozen review hints

journal.cli.talent.daily.review.inputs = missing frozen review admission inputs

journal.cli.talent.daily.review.merges = review output lacks merges

journal.cli.talent.daily.review.name = frozen candidate lacks name

journal.cli.talent.daily.review.owner = missing frozen review owner state

journal.cli.talent.daily.review.promotion = missing frozen promotion owner state

journal.cli.talent.daily.review.promotion.changed = conflict: promotion owner state changed after prompt preparation

journal.cli.talent.daily.review.promotions = review output lacks promotions

journal.cli.talent.daily.review.proposals = missing frozen proposal state

journal.cli.talent.daily.review.proposals.changed = conflict: merge proposal changed after prompt preparation

journal.cli.talent.daily.review.type = frozen candidate lacks type

journal.cli.talent.daily.type = unsupported talent type: { $kind }

journal.cli.talent.daily.validation.aliases = validation: promoted review candidate must include aliases array

journal.cli.talent.daily.validation.json = validation: invalid { $shape } JSON: { $error }

journal.cli.talent.daily.validation.maintenance.missing = validation: maintenance primary is missing

journal.cli.talent.daily.validation.maintenance.time = validation: maintenance primary must be HH:MM

journal.cli.talent.daily.validation.merges = validation: review merges must be an array

journal.cli.talent.daily.validation.observer = validation: entities:entity_observer requires an entities array

journal.cli.talent.daily.validation.promotions = validation: review promotions must be an array

journal.cli.talent.daily.validation.suggest = validation: suggest entities must be an array

journal.cli.talent.daily.write.contract = unsupported daily write contract for { $talent }

journal.cli.talent.outcome.facet.write = FacetEntityWriteError: { $error }

journal.cli.talent.outcome.object = result is not a JSON object

journal.cli.talent.outcome.observation.write = ObservationWriteError: { $error }

journal.cli.talent.outcome.observer.entities = entities is not a list

journal.cli.talent.outcome.observer.parse = could not parse result as JSON

journal.cli.talent.outcome.review.arrays = result missing expected arrays

journal.cli.talent.outcome.review.candidate = EntityReviewCandidateError: { $error }

journal.cli.talent.outcome.review.parse = invalid JSON

journal.cli.talent.daily.owner.alias.claimed = conflict: promotion alias was claimed after preparation

journal.cli.talent.daily.owner.alias.target.detached = conflict: alias target is no longer attached

journal.cli.talent.daily.owner.identity.blocked.facet = conflict: promotion name hidden in this facet

journal.cli.talent.daily.owner.identity.blocked.hidden = conflict: the entity is hidden, so its names were not changed

journal.cli.talent.daily.owner.identity.blocked.identity = conflict: promotion identity hidden

journal.cli.talent.daily.owner.identity.changed = conflict: promoted identity changed after preparation

journal.cli.talent.daily.owner.identity.deleted = conflict: promoted identity was deleted after preparation

journal.cli.talent.daily.owner.identity.disappeared = conflict: promotion identity disappeared

journal.cli.talent.daily.owner.identity.map.group.lost = conflict: promotion { $name } matches { $directory }, which lost its identity-map group

journal.cli.talent.daily.owner.identity.merged = conflict: promoted identity was merged away after preparation

journal.cli.talent.daily.owner.identity.moved = conflict: promoted identity moved after preparation

journal.cli.talent.daily.owner.identity.unreadable = conflict: promotion { $name } derives entity id { $slug }, whose identity { $directory } cannot be read

journal.cli.talent.daily.owner.merge.proposals.changed = conflict: merge proposals changed after preparation

journal.cli.talent.daily.owner.name.matches.multiple = conflict: promotion { $name } matches { $count } identities by name

journal.cli.talent.daily.owner.no.usable.entity.id = conflict: promotion { $name } has no usable entity id

journal.cli.talent.daily.owner.owning.facet.changed.absent = conflict: owning facet no longer exists

journal.cli.talent.daily.owner.owning.facet.changed.invalid = conflict: owning facet has no valid stable identity

journal.cli.talent.daily.owner.owning.facet.changed.replaced = conflict: owning facet was replaced after preparation

journal.cli.talent.daily.owner.principal.grant.refused.exists = conflict: a journal owner already exists, so this person is not marked

journal.cli.talent.daily.owner.principal.grant.refused.name = conflict: the configured owner name no longer matches this person

journal.cli.talent.daily.owner.principal.settings = the journal settings could not be read, so this person was not marked as the owner

journal.cli.talent.daily.owner.promoted.entity.blocked.hidden = conflict: promoted entity is hidden

journal.cli.talent.daily.owner.promoted.entity.blocked.name = conflict: promoted entity { $name } is hidden

journal.cli.talent.daily.owner.relationship.changed = conflict: promotion relationship changed after preparation

journal.cli.talent.daily.owner.relationship.occupied = conflict: promotion { $name } needs relationship directory { $relationship_dir }, which is already occupied

journal.cli.importer.plaud.connect.open = open this link to sign in with { $provider }:

journal.cli.importer.plaud.connect.waiting = waiting for { $provider } to finish sign-in. if your browser is on another device, paste the final address from the browser bar and press enter:

journal.cli.importer.plaud.connect.retry.url = that wasn't the sign-in address from { $provider }. paste the full address from your browser bar, or finish in the browser.

journal.cli.importer.plaud.connect.done = { $provider } is connected, and your sign-in is saved in this journal.

journal.cli.importer.plaud.connect.done.account = { $provider } is connected as { $account }, and your sign-in is saved in this journal.

journal.cli.importer.plaud.connect.next = run { $command } to bring in recordings now, or turn on hourly sync in settings.

journal.cli.importer.plaud.connect.failed = could not connect { $provider }: { $detail }

journal.cli.importer.plaud.sync.busy = your journal is already checking { $provider }. try again when it finishes.

journal.cli.sol.dispatch.unsupported.command = unsupported command.

journal.cli.sol.help.authority = Authority: { $path }

journal.cli.sol.help.command.activities.create = Create a new synthetic activity record from { $argv } flags or { $json } on { $stdin }.

journal.cli.sol.help.command.activities.get = Fetch one activity record by ID.

journal.cli.sol.help.command.activities.list = List activity records for one day or an inclusive day range.

journal.cli.sol.help.command.activities.mute = Hide an activity record without deleting it.

journal.cli.sol.help.command.activities.unmute = Restore a previously hidden activity record.

journal.cli.sol.help.command.activities.update = Apply an { $argv } flag or { $stdin } { $json } patch to one activity record.

journal.cli.sol.help.command.awareness.imports = Read or update import tracking state.

journal.cli.sol.help.command.awareness.log = Append an entry to the daily awareness log.

journal.cli.sol.help.command.awareness.log.read = Read entries from the daily awareness log.

journal.cli.sol.help.command.awareness.status = Show current awareness state.

journal.cli.sol.help.command.body.day = Show one day of imported health data.

journal.cli.sol.help.command.body.status = Show imported health data status.

journal.cli.sol.help.command.body.window = Show imported health context for a time window.

journal.cli.sol.help.command.entities.accept.merge.candidate = Preview or accept one recorded entity merge candidate.

journal.cli.sol.help.command.entities.aka = Add an alias to an attached entity.

journal.cli.sol.help.command.entities.ambiguities = List reviewable entity-resolution ambiguities.

journal.cli.sol.help.command.entities.attach = Attach an entity permanently to a facet.

journal.cli.sol.help.command.entities.delete = Move an entity and its facet links to the trash; { $erase_flag } erases it at once. With { $erase_flag }, { $from_backups_flag } also removes it from your backups and { $not_from_backups_flag } leaves it there. Without either, the trash setting decides.

journal.cli.sol.help.command.entities.detect = Record a detected entity for a day in a facet.

journal.cli.sol.help.command.entities.dismiss.merge.candidate = Dismiss one recorded entity merge candidate.

journal.cli.sol.help.command.entities.entity.history = Show durable identity versions (not relationship history).

journal.cli.sol.help.command.entities.history = Show recorded connection evidence for an entity pair.

journal.cli.sol.help.command.entities.list = List entities for a facet.

journal.cli.sol.help.command.entities.merge = Plan or commit a journal-entity merge.

journal.cli.sol.help.command.entities.merge.candidates = List recorded entity merge candidates.

journal.cli.sol.help.command.entities.move = Move an entity from one facet to another.

journal.cli.sol.help.command.entities.network = Show one-hop recorded connections for an entity.

journal.cli.sol.help.command.entities.observations = List observations for an attached entity.

journal.cli.sol.help.command.entities.observe = Add an observation to an attached entity.

journal.cli.sol.help.command.entities.overview = Show a global recorded-connections overview.

journal.cli.sol.help.command.entities.record.merge.candidate = Record a proposed entity merge (source variant -> canonical target).

journal.cli.sol.help.command.entities.resolve.ambiguity = Resolve or replace one persisted ambiguity choice.

journal.cli.sol.help.command.entities.resolve.ambiguity.group = Resolve an ambiguity card group for an entity candidate.

journal.cli.sol.help.command.entities.restore.review = Restore one set-aside review item or undo an automatic choice.

journal.cli.sol.help.command.entities.restore.version = Restore one ordinary identity version.

journal.cli.sol.help.command.entities.review = List grouped entity review cards and set-aside items.

journal.cli.sol.help.command.entities.search = Search entities by text, type, facet, or activity.

journal.cli.sol.help.command.entities.sweep.review = Run the entity review policy sweep.

journal.cli.sol.help.command.entities.update = Update an entity description.

journal.cli.sol.help.command.facets.accept = Accept one facet review candidate.

journal.cli.sol.help.command.facets.dismiss = Dismiss one facet review candidate.

journal.cli.sol.help.command.facets.list.candidates = List recorded facet review candidates.

journal.cli.sol.help.command.health.for.range = Render the journal-data trust report for an inclusive day range.

journal.cli.sol.help.command.health.full = Render the full journal-data trust report for one day.

journal.cli.sol.help.command.health.pipeline = Thin wrapper around { $pipeline_module }. For journal-data trust checks use `{ $summary_verb }` / `{ $full_verb }` / `{ $for_range_verb }`.

journal.cli.sol.help.command.health.summary = Summarize journal-data trust signals for one day.

journal.cli.sol.help.command.import.top.level = Import media through the journal

journal.cli.sol.help.command.journal.agents = List available agent outputs for a day.

journal.cli.sol.help.command.journal.facet.create = Create a new facet.{ $line_break }{ $line_break }{ $consent_flag } records in the facet's audit log that the owner approved this. An{ $line_break }agent passes it after the owner approves a change the agent proposed, not for{ $line_break }one the owner asked for. It does not change what is created.

journal.cli.sol.help.command.journal.facet.delete = Move a facet and everything in it to the trash, where it can be recovered until it is erased.{ $line_break }{ $line_break }{ $yes_flag } is required; without it the command refuses. Every delete is recorded in{ $line_break }the journal's action log as owner-approved, and { $consent_flag } is accepted but changes{ $line_break }nothing.

journal.cli.sol.help.command.journal.facet.mute = Mute a facet (hide from default listings).

journal.cli.sol.help.command.journal.facet.rename = Rename a facet.{ $line_break }{ $line_break }{ $consent_flag } records in the facet's audit log that the owner approved this. An{ $line_break }agent passes it after the owner approves a change the agent proposed, not for{ $line_break }one the owner asked for. It does not change the rename.

journal.cli.sol.help.command.journal.facet.show = Show facet summary.

journal.cli.sol.help.command.journal.facet.unmute = Unmute a facet (show in default listings).

journal.cli.sol.help.command.journal.facet.update = Update facet configuration.

journal.cli.sol.help.command.journal.facets = List facets.

journal.cli.sol.help.command.journal.import = Show full metadata for a single import.

journal.cli.sol.help.command.journal.imports = List recent imports with metadata.

journal.cli.sol.help.command.journal.read = Read full content of an agent output.

journal.cli.sol.help.command.journal.retention.config = Show or update retention configuration.

journal.cli.sol.help.command.journal.retention.list = List original media ready for removal.

journal.cli.sol.help.command.journal.search = Search the journal index.{ $line_break }{ $line_break }Use 2-4 content terms instead of natural-language questions; question words{ $line_break }like { $example_terms } add noise in this keyword/{ $bm25 } index. Syntax: { $or_operator } for{ $line_break }any term, quoted phrases for exact text, and { $wildcard } for prefix matches.{ $line_break }{ $n_flag }/{ $limit_flag } is the maximum results per day. Results are grouped by day,{ $line_break }newest first, up to 20 days per page. { $offset_flag } skips days. { $d_flag }/{ $day_flag } is one{ $line_break }exact day and cannot be combined with { $day_from_flag } or { $day_to_flag }. When nothing{ $line_break }matches every term within the other filters (facet, agent, stream, time{ $line_break }bucket) across the whole journal, the response is marked { $relaxed } and those{ $line_break }hits are weaker matches; that judgment happens before the date filter, so an{ $line_break }empty day or range does not mean the range never mentions the terms. Broaden{ $line_break }by dropping terms, then { $or_operator }, then { $wildcard }. { $json_flag } does not change the output{ $line_break }because it is { $json } either way.{ $line_break }Read one result as last indexed with `{ $command }`. Copy all three fields from the same result; do not use its display id as a file path.

journal.cli.sol.help.command.journal.storage.summary = Show journal storage summary.

journal.cli.sol.help.command.link.authorized.clients = List every authorized client cert: fingerprint, label, last-seen (flat view).

journal.cli.sol.help.command.link.confirm = confirm the journal's mark for a link waiting on it

journal.cli.sol.help.command.link.join = join a { $solstone } with a short code or pair link

journal.cli.sol.help.command.link.list = Print every paired device with its last-seen time.

journal.cli.sol.help.command.link.local.network.close = Close your journal to devices on your network.

journal.cli.sol.help.command.link.local.network.open = Open your journal to devices on your network.

journal.cli.sol.help.command.link.local.network.status = Show whether your journal is open to devices on your network.

journal.cli.sol.help.command.link.observer.pause = pause linked devices (not yet available).

journal.cli.sol.help.command.link.pair = Start a pairing link, print join-ready credentials, optionally wait.

journal.cli.sol.help.command.link.private.link.disable = Turn off private network.

journal.cli.sol.help.command.link.private.link.setup = Set up private network.

journal.cli.sol.help.command.link.private.link.status = Show private network status.

journal.cli.sol.help.command.link.serve = serve a paired journal over the local link bridge

journal.cli.sol.help.command.link.set.label = Set or clear the owner label for a paired device.

journal.cli.sol.help.command.link.status = Report enrollment, { $listen_ws } state, active tunnel count, relay endpoint.

journal.cli.sol.help.command.link.top.level.status = show link status and observed remote journal version

journal.cli.sol.help.command.link.unpair = Revoke a paired device. Next reconnect from that device fails at { $tls } handshake.

journal.cli.sol.help.command.moved.navigate = Moved to `{ $command }`.

journal.cli.sol.help.command.settings.convey.status = Show { $convey } network and dashboard URL status.

journal.cli.sol.help.command.settings.identity.set = Update journal owner identity.

journal.cli.sol.help.command.settings.identity.show = Show journal identity config.

journal.cli.sol.help.command.settings.keys.clear = Clear an API key from journal config.

journal.cli.sol.help.command.settings.keys.set = Set an API key in journal config.

journal.cli.sol.help.command.settings.keys.show = Show configured API key status.

journal.cli.sol.help.command.settings.keys.validate = Validate all configured API keys without persisting by default.

journal.cli.sol.help.command.settings.processing.set = Set deferred-processing mode and drain window.

journal.cli.sol.help.command.settings.processing.show = Show effective deferred-processing settings.

journal.cli.sol.help.command.settings.show = Show a summary of journal settings.

journal.cli.sol.help.command.settings.transcribe.set.backend = Set the transcription backend.

journal.cli.sol.help.command.settings.transcribe.show = Show transcription backend configuration.

journal.cli.sol.help.command.speakers.attribute.segment = Run speaker attribution (Layers 1-3) on a single segment.{ $line_break }{ $line_break }Classifies each sentence using owner detection, structural heuristics,{ $line_break }and acoustic voiceprint matching.  Optionally writes { $labels_file }{ $line_break }and accumulates high-confidence voiceprints.

journal.cli.sol.help.command.speakers.backfill = Run speaker attribution across all segments with embeddings.{ $line_break }{ $line_break }Processes segments oldest-first for progressive voiceprint building.{ $line_break }Skips segments that already have { $labels_file } (safe to re-run).

journal.cli.sol.help.command.speakers.backfill.last.seen = Backfill { $last_seen_field } on existing voiceprint metadata rows.

journal.cli.sol.help.command.speakers.backfill.resume = Resume an interrupted or failed speaker attribution backfill operation.

journal.cli.sol.help.command.speakers.backfill.status = Show status of a speaker attribution backfill operation.

journal.cli.sol.help.command.speakers.bootstrap = Bootstrap voiceprints from single-speaker segments.{ $line_break }{ $line_break }Scans the full journal for segments where { $speakers_file } lists exactly{ $line_break }one speaker. In those segments, all non-owner embeddings belong to that{ $line_break }speaker. Saves them as voiceprints using the owner centroid for{ $line_break }owner subtraction.

journal.cli.sol.help.command.speakers.build.from.tags = Build the owner voice centroid from validated manual owner tags.

journal.cli.sol.help.command.speakers.confirm.owner = Confirm the owner voice candidate in the browser.{ $line_break }{ $line_break }Native { $confirm_verb } does not save a centroid. Review and confirm candidates in the browser at { $speakers_page }.

journal.cli.sol.help.command.speakers.correct = Correct one speaker attribution immediately.

journal.cli.sol.help.command.speakers.day.segments = List speaker-review segments for a day.

journal.cli.sol.help.command.speakers.detect = Run owner voice candidate detection.

journal.cli.sol.help.command.speakers.discover = Discover recurring unknown speakers across segments.

journal.cli.sol.help.command.speakers.dismiss.cluster = Dismiss a current discovery cluster from listing surfaces.

journal.cli.sol.help.command.speakers.dismissals = List folded speaker cluster dismissals.

journal.cli.sol.help.command.speakers.identify = Identify a discovered unknown speaker cluster.

journal.cli.sol.help.command.speakers.identify.operation = Show one redacted speaker identify operation.

journal.cli.sol.help.command.speakers.identify.operations = List redacted speaker identify operations.

journal.cli.sol.help.command.speakers.identify.undo = Undo a committed speaker identify operation.

journal.cli.sol.help.command.speakers.keep.separate.list = List folded speaker keep-separate assertions.

journal.cli.sol.help.command.speakers.link.import = Link an import participant name as an aka on an existing entity.

journal.cli.sol.help.command.speakers.merge.names = Merge a speaker name variant into a canonical entity.

journal.cli.sol.help.command.speakers.owner.ready = Check if owner voice detection should be surfaced to the user.

journal.cli.sol.help.command.speakers.presence = Show who was around a discovered unknown voice cluster.

journal.cli.sol.help.command.speakers.propagate.correction = Preview or apply scoped correction propagation.{ $line_break }{ $line_break }Reversal uses the same verb with the two speaker ids swapped.

journal.cli.sol.help.command.speakers.rebuild.owner = Rebuild the owner voice centroid from current manual owner tags.

journal.cli.sol.help.command.speakers.reject.owner = Reject the rendered owner voice candidate generation.

journal.cli.sol.help.command.speakers.repair = Repair non-{ $person_type } speaker contamination without relearning from it.{ $line_break }{ $line_break }Surveys identities and voiceprints, removes non-{ $person_type } voiceprint contamination,{ $line_break }and updates segment labels using fresh attribution under current state.

journal.cli.sol.help.command.speakers.repair.resume = Resume an interrupted or failed speaker repair operation.

journal.cli.sol.help.command.speakers.repair.status = Show status of a speaker repair operation.

journal.cli.sol.help.command.speakers.resolve.names = Resolve speaker name variants using voiceprint similarity.{ $line_break }{ $line_break }Compares voiceprint centroids between all entities. Pairs with cosine{ $line_break }similarity > 0.90 are flagged as the same person. Unambiguous variants{ $line_break }(short name is first word of full name) are auto-merged by adding the{ $line_break }short name as an aka on the canonical entity.

journal.cli.sol.help.command.speakers.seed.from.imports = Seed voiceprints from import segments with speaker-attributed transcripts.{ $line_break }{ $line_break }Scans import streams for segments with both { $transcript_file }{ $line_break }(with speaker labels) and audio embeddings. Maps each embedding to a speaker{ $line_break }via time-based alignment, matches speakers to existing entities, and saves{ $line_break }embeddings as voiceprints with owner contamination guard.

journal.cli.sol.help.command.speakers.sentences = List sentence ids for one speaker-review source.

journal.cli.sol.help.command.speakers.status = Show speaker subsystem status as { $json }.

journal.cli.sol.help.command.speakers.suggest = Suggest speaker curation opportunities.

journal.cli.sol.help.command.speakers.tag.owner = Tag one sentence as the owner voice.

journal.cli.sol.help.command.speakers.wipe = Move every speaker voice file in the journal to the trash.{ $line_break }{ $line_break }Without { $commit_flag }, prints a report of what would be moved. With{ $line_break }{ $commit_flag }, moves segment-embedding NPZs, speaker labels/corrections,{ $line_break }per-entity voiceprints, owner centroids, the owner-candidate snapshot{ $line_break }and the candidate pool to the trash, where they can be recovered until{ $line_break }they are erased. { $erase_flag } erases them at once instead.{ $line_break }{ $line_break }With { $erase_flag }, { $from_backups_flag } also removes them from your backups and{ $line_break }{ $not_from_backups_flag } leaves them there. Without either, the trash{ $line_break }setting decides.

journal.cli.sol.help.command.status.top.level = Show journal network status.

journal.cli.sol.help.command.thinking.clear.local.endpoint = Clear the BYO local provider endpoint.

journal.cli.sol.help.command.thinking.confidential.disable = Disable confidential processing.

journal.cli.sol.help.command.thinking.confidential.enable = Enable confidential processing.

journal.cli.sol.help.command.thinking.confidential.recheck = Recheck confidential processing.

journal.cli.sol.help.command.thinking.confidential.status = Show confidential processing status.

journal.cli.sol.help.command.thinking.keys.clear = Clear an AI key from journal config.

journal.cli.sol.help.command.thinking.keys.set = Set an AI key in journal config.

journal.cli.sol.help.command.thinking.keys.show = Show configured AI key status.

journal.cli.sol.help.command.thinking.keys.validate = Validate configured AI keys.

journal.cli.sol.help.command.thinking.local.availability = Show local model availability.

journal.cli.sol.help.command.thinking.local.bootstrap = Start local model setup.

journal.cli.sol.help.command.thinking.local.bootstrap.cancel = Cancel local model setup.

journal.cli.sol.help.command.thinking.local.bootstrap.status = Show local setup status.

journal.cli.sol.help.command.thinking.local.models = List local models.

journal.cli.sol.help.command.thinking.local.readiness = Show local provider readiness.

journal.cli.sol.help.command.thinking.local.status = Show local provider status.

journal.cli.sol.help.command.thinking.providers.set.active = Set the provider and model used for all thinking.

journal.cli.sol.help.command.thinking.providers.show = Show provider configuration.

journal.cli.sol.help.command.thinking.set.local.endpoint = Set the BYO local provider endpoint.

journal.cli.sol.help.command.transcripts.read = Read transcript content for a day, segment, or time range.

journal.cli.sol.help.command.transcripts.scan = List transcript coverage ranges for a day.

journal.cli.sol.help.command.transcripts.segments = List recording segments for a day.

journal.cli.sol.help.command.transcripts.speakers = List speaker correction rows for one transcript segment.

journal.cli.sol.help.command.transcripts.stats = Show daily transcript coverage counts for a month.

journal.cli.sol.help.command.usage = Usage: { $syntax }

journal.cli.sol.help.operation = Operation: { $operation }

journal.cli.sol.help.parameter = { $shape ->
    *[count] { $syntax } [count]
    [multiple] { $syntax } [multiple]
    [multiple_count] { $syntax } [multiple, count]
    [required] { $syntax } [required]
    [required_count] { $syntax } [required, count]
    [required_multiple] { $syntax } [required, multiple]
    [required_multiple_count] { $syntax } [required, multiple, count]
    }

journal.cli.sol.help.parameter.default = { $shape ->
    *[plain] { $syntax } [default: { $default_value }]
    [count] { $syntax } [default: { $default_value }, count]
    [multiple] { $syntax } [default: { $default_value }, multiple]
    [multiple_count] { $syntax } [default: { $default_value }, multiple, count]
    [required] { $syntax } [required, default: { $default_value }]
    [required_count] { $syntax } [required, default: { $default_value }, count]
    [required_multiple] { $syntax } [required, default: { $default_value }, multiple]
    [required_multiple_count] { $syntax } [required, default: { $default_value }, multiple, count]
    }

journal.cli.sol.help.parameter.default.flag = { $shape ->
    *[plain] { $syntax } [default: { $default_value }, flag value: { $flag_value }]
    [count] { $syntax } [default: { $default_value }, flag value: { $flag_value }, count]
    [multiple] { $syntax } [default: { $default_value }, flag value: { $flag_value }, multiple]
    [multiple_count] { $syntax } [default: { $default_value }, flag value: { $flag_value }, multiple, count]
    [required] { $syntax } [required, default: { $default_value }, flag value: { $flag_value }]
    [required_count] { $syntax } [required, default: { $default_value }, flag value: { $flag_value }, count]
    [required_multiple] { $syntax } [required, default: { $default_value }, flag value: { $flag_value }, multiple]
    [required_multiple_count] { $syntax } [required, default: { $default_value }, flag value: { $flag_value }, multiple, count]
    }

journal.cli.sol.help.parameter.flag = { $shape ->
    *[plain] { $syntax } [flag value: { $flag_value }]
    [count] { $syntax } [flag value: { $flag_value }, count]
    [multiple] { $syntax } [flag value: { $flag_value }, multiple]
    [multiple_count] { $syntax } [flag value: { $flag_value }, multiple, count]
    [required] { $syntax } [required, flag value: { $flag_value }]
    [required_count] { $syntax } [required, flag value: { $flag_value }, count]
    [required_multiple] { $syntax } [required, flag value: { $flag_value }, multiple]
    [required_multiple_count] { $syntax } [required, flag value: { $flag_value }, multiple, count]
    }

journal.cli.sol.help.section.arguments = Arguments:

journal.cli.sol.help.section.commands = Commands:

journal.cli.sol.help.section.options = Options:

journal.cli.install.archive.binary = { $binary_name } missing from archive

journal.cli.install.archive.digest = { $algorithm } mismatch: expected { $expected }, got { $actual }

journal.cli.install.archive.download = download failed: { $message }

journal.cli.install.archive.host = download host refused: { $host }

journal.cli.install.archive.origin = download origin unavailable at { $host }: { $message }

journal.cli.install.archive.path = archive member escapes destination: { $path }

journal.cli.install.archive.redirect = download redirect hop limit exceeded: { $limit }

journal.cli.install.archive.scheme = download scheme refused for { $host }: { $scheme }

journal.cli.install.archive.size = { $quantity ->
    [one] download size mismatch: expected { $expected } byte, got { $actual }
   *[other] download size mismatch: expected { $expected } bytes, got { $actual }
}

journal.cli.install.archive.userinfo = download URL authority must not include userinfo: { $authority }

journal.cli.install.backend.metal = backend must be "metal" when supplied

journal.cli.install.backend.unavailable = the requested local backend is unavailable on this platform

journal.cli.install.component.packaged = build defect: this component ships inside the package, and retrying will not help

journal.cli.install.component.unfetched = build defect: this build does not download this component, and retrying will not help

journal.cli.install.coreml.asset.digest = parakeet { $coreml } asset digest mismatch for { $file }: expected { $expected }, found { $found }

journal.cli.install.coreml.asset.incomplete = speech model asset is incomplete

journal.cli.install.coreml.asset.missing = parakeet { $coreml } asset missing: { $file }; install it with: { $install_command }

journal.cli.install.coreml.asset.read = failed to read { $file }: { $error }

journal.cli.install.coreml.asset.size = parakeet { $coreml } asset size mismatch for { $file }: expected { $expected }, found { $found }

journal.cli.install.coreml.attempt.changed = the speech install attempt changed.

journal.cli.install.coreml.cache.directory = parakeet { $coreml } sentinel cache dir ({ $installed }) does not match configured ({ $configured })

journal.cli.install.coreml.catalog.conflict = { $artifact } catalog rows have conflicting versions

journal.cli.install.coreml.catalog.empty = no { $artifact } catalog rows found

journal.cli.install.coreml.catalog.mismatch = parakeet { $coreml } catalog version mismatch: found { $found }, expected { $expected }

journal.cli.install.coreml.platform = parakeet { $coreml } is only supported on { $host }, not { $os_name }/{ $arch }

journal.cli.install.coreml.publication.lock = failed to acquire publication lock: { $error }

journal.cli.install.coreml.publish.restore = { $error }; restore failed: { $restore }

journal.cli.install.coreml.sentinel.not.ready = parakeet { $coreml } sentinel not ready: { $path }; install it with: { $install_command }

journal.cli.install.coreml.sentinel.removed = incumbent sentinel was removed before publication

journal.cli.install.cuda.integrity = cuda runtime integrity failure

journal.cli.install.disk.insufficient = not enough space to finish downloading the model

journal.cli.install.fingerprint.artifact = { $field } missing

journal.cli.install.fingerprint.backend = backend missing

journal.cli.install.fingerprint.missing = attempt fingerprint missing

journal.cli.install.fingerprint.runtime = runtime artifact key missing

journal.cli.install.hardware.apple.memory = local thinking needs at least { $minimum } of memory

journal.cli.install.hardware.memory.minimum = { $available_bytes ->
    [one] device-local { $vram } below { $minimum } minimum: { $available_bytes } byte
   *[other] device-local { $vram } below { $minimum } minimum: { $available_bytes } bytes
}

journal.cli.install.hardware.memory.unknown = { $vulkan } probe failed to report memory

journal.cli.install.hardware.no.hardware = no hardware { $vulkan } GPU found

journal.cli.install.hardware.probe.required = { $vulkan } GPU observation required on Windows

journal.cli.install.lease.held = { $provider } install lease is held

journal.cli.install.lease.regular = install lease is not a regular file

journal.cli.install.member.cuda.arch = unsupported cuda arch { $arch }

journal.cli.install.member.cuda.arch.key = unsupported cuda arch key { $artifact_key }

journal.cli.install.member.cuda.missing = required cuda member missing: { $name }

journal.cli.install.member.cuda.regular = required cuda member is not a regular file: { $name }

journal.cli.install.member.link.broken = broken link in chain for { $name }

journal.cli.install.member.link.cycle = link cycle detected for { $name }

journal.cli.install.member.link.missing = required link member missing: { $name }

journal.cli.install.member.link.read = cannot read link { $name }: { $error }

journal.cli.install.member.link.regular = link { $name } does not resolve to regular file

journal.cli.install.member.link.target = link { $name } target mismatch: expected { $target }, got { $found }

journal.cli.install.member.link.type = required link member is not a symlink: { $name }

journal.cli.install.member.link.unreadable = cannot read link in chain for { $name }

journal.cli.install.member.regular.missing = required regular member missing: { $name }

journal.cli.install.member.regular.type = required regular member is not a regular file: { $name }

journal.cli.install.metal.platform = { $metal } local inference is unsupported on { $key }

journal.cli.install.mlx.digest.read = { $function } failed for { $file }: { $error }

journal.cli.install.mlx.directory.explicit = explicit { $mlx } model directory does not exist: { $path }

journal.cli.install.mlx.directory.invalid = model dir is not a directory: { $path }

journal.cli.install.mlx.directory.override = { $env } does not exist: { $path }

journal.cli.install.mlx.directory.read = cannot read model directory: { $error }

journal.cli.install.mlx.directory.unset = { $env } is unset or not a directory

journal.cli.install.mlx.entry.read = failed reading entry: { $error }

journal.cli.install.mlx.file.digest = sha256 mismatch for { $file }: expected { $expected } got { $actual }

journal.cli.install.mlx.file.missing = missing required file: { $file }

journal.cli.install.mlx.file.regular = not a regular file: { $file }

journal.cli.install.mlx.file.size = size mismatch for { $file }: expected { $expected } got { $actual }

journal.cli.install.mlx.helper.explicit = explicit { $mlx } helper is not valid or missing { $library }: { $path }

journal.cli.install.mlx.helper.invalid = helper binary is invalid: { $path }

journal.cli.install.mlx.helper.missing = { $helper } executable with { $library } resource bundle not found

journal.cli.install.mlx.helper.override = { $env } is not valid or missing { $library }: { $path }

journal.cli.install.mlx.input.extra = extra unverified model input: { $file }

journal.cli.install.mlx.metadata.read = cannot read metadata for { $file }: { $error }

journal.cli.install.mlx.override.failed = { $mlx } override verification failed

journal.cli.install.mlx.pins.empty = empty pins list

journal.cli.install.mlx.pins.missing = no pins supplied

journal.cli.install.mlx.platform = { $mlx } requires { $os }

journal.cli.install.mlx.type.read = failed file type: { $error }

journal.cli.install.pin.missing = no pin for { $key }

journal.cli.install.registry.artifact.ambiguous = catalog has multiple matching { $unit } artifacts

journal.cli.install.registry.artifact.missing = catalog has no matching { $unit } artifact

journal.cli.install.registry.platform = no catalog platform mapping for { $key }

journal.cli.install.runtime.unavailable = the journal app does not provide this local runtime

journal.cli.install.speech.platform = { $program } is unsupported on { $os_name }/{ $arch }

journal.cli.install.staging.binary.parent = binary has no parent

journal.cli.install.staging.bundle.occupied = bundle destination already exists: { $path }

journal.cli.install.staging.target.name = target has no file name

journal.cli.install.staging.target.parent = target has no parent

journal.cli.install.status.conflict.different.attempt.while.install.in.flight = install status conflict: different attempt while install in-flight

journal.cli.install.status.conflict.illegal.install.status.transition = install status conflict: illegal install status transition

journal.cli.install.status.conflict.install.attempt.id.changed = install status conflict: install attempt id changed

journal.cli.install.status.conflict.install.attempt.is.no.longer.in.flight = install status conflict: install attempt is no longer in-flight

journal.cli.install.status.conflict.install.progress.can.only.be.bumped.for = install status conflict: install progress can only be bumped for in-flight states

journal.cli.install.status.conflict.install.target.fingerprint.changed = install status conflict: install target fingerprint changed

journal.cli.install.status.conflict.interrupted.attempt.id.does.not.match = install status conflict: interrupted attempt id does not match

journal.cli.install.status.conflict.interrupted.target.fingerprint.does.not.match = install status conflict: interrupted target fingerprint does not match

journal.cli.install.status.conflict.new.in.flight.attempt.reused.attempt.id = install status conflict: new in-flight attempt reused attempt id

journal.cli.install.status.conflict.non.idle.install.status.requires.attempt.id = install status conflict: non-idle install status requires attempt id

journal.cli.install.status.conflict.only.in.flight.installs.can.be.interrupted = install status conflict: only in-flight installs can be interrupted

journal.cli.install.status.conflict.stale.install.status.revision = install status conflict: stale install status revision

journal.cli.install.status.lock = status lock error: { $error }

journal.cli.install.status.malformed.file = malformed install status: { $path }

journal.cli.install.status.malformed.initial.install.attempt.state.must.be.in = malformed install status: initial install attempt state must be in-flight

journal.cli.install.status.malformed.invalid.provider = malformed install status: invalid status for { $provider }

journal.cli.install.status.malformed.owner.must.be.an.object.or.null = malformed install status: owner must be an object or null

journal.cli.install.status.malformed.providers = malformed install status: provider install status must be one of: [{ $listed }]

journal.cli.install.status.malformed.state = malformed install status: unknown install state: { $state }

journal.cli.install.terminal.write = native install failed and failed to write status

journal.cli.install.windows.digest.absent = signed payload manifest does not declare digest for { $path }

journal.cli.install.windows.executable.location = running journal executable is not in the package bin directory: { $path }

journal.cli.install.windows.executable.parent = running journal executable has no containing directory

journal.cli.install.windows.executable.read = could not determine the running journal executable: { $error }

journal.cli.install.windows.package.root = package bin directory has no package root

journal.cli.install.windows.package.runtime = Windows local thinking package verification requires a Windows runtime

journal.cli.install.windows.platform = unsupported Windows platform: { $key }

journal.cli.install.windows.target.different = signed payload target differs from { $key }

journal.cli.install.windows.target.unsupported = signed payload target is unsupported

journal.cli.client.date.short = { $weekday } { $month } { $day }

journal.cli.client.date.short.year = { $weekday } { $month } { $day } '{ $year }

journal.cli.client.erase.backups.conflict = Error: choose { $from_backups } or { $skip_backups }, not both.

journal.cli.client.erase.backups.missing = Error: backup isn't set up, so there are no backups to remove it from.

journal.cli.client.erase.backups.next = Removing it from your backups next.

journal.cli.client.erase.backups.resume = Removing it from your backups once you resume backup.

journal.cli.client.erase.backups.unavailable = Error: this journal can't remove things from your backups yet.

journal.cli.client.erase.backups.unchecked = Error: your journal couldn't check your backups just now. Try again, or pass { $skip_backups } to erase without removing it from your backups.

journal.cli.client.error.detail = Error: { $detail }

journal.cli.client.lock.acquire = failed to acquire sidecar lock: { $error }

journal.cli.client.lock.open = cannot open lock file: { $error }

journal.cli.client.missing.argument = Error: missing argument { $argument }

journal.cli.client.network.cli.pair.join.hint = link this device with:

journal.cli.client.network.cli.pair.local.network.closed = can't start pairing. your journal is closed to devices on your network. run `{ $call_open }` to pair another device over your network, or turn on your private network to pair through the relay.

journal.cli.client.network.cli.pair.no.lan.address = can't start pairing. your journal isn't reachable on a network address yet. turn on your private network to pair from anywhere, or connect this device to your home network.

journal.cli.client.network.clients.empty = No authorized clients.

journal.cli.client.network.clients.seen = { $fingerprint }  { $label }  last seen { $when }

journal.cli.client.network.device.fingerprint.absent = No paired device with fingerprint { $fingerprint }

journal.cli.client.network.device.label.absent = No paired device with label { $label }

journal.cli.client.network.device.label.cleared = Device label set to: (cleared)

journal.cli.client.network.device.label.set = Device label set to: { $label }

journal.cli.client.network.devices.empty = No devices linked yet.

journal.cli.client.network.devices.seen = - { $label } · added { $added } · last seen { $seen } [{ $fingerprint }]

journal.cli.client.network.devices.total = Paired devices: { $total }

journal.cli.client.network.journal.id = journal id:     { $id }

journal.cli.client.network.label.options = Error: specify either { $label } or { $clear }

journal.cli.client.network.linked.systems.heading = Linked systems:

journal.cli.client.network.listen.state = { $protocol } state: (query { $convey } { $path } for live state)

journal.cli.client.network.local.network.open = your journal is open to devices on your network. devices on the same network can pair with it directly.

journal.cli.client.network.local.network.this.pc = your journal is closed to devices on your network. the { $brand } app on this computer still pairs, and so do devices through the relay, once your private network is on.

journal.cli.client.network.local.network.windows.asks = windows may ask whether journal can use your network. on a standard account, an administrator has to allow it. if windows didn't ask and a device still can't reach your journal, an earlier choice in windows may be blocking journal. an administrator can allow it in { $security }, under { $firewall }.

journal.cli.client.network.mark.none = your journal's identity is created when its mark is locked in during setup.

journal.cli.client.network.mark.unavailable = your journal's mark: unavailable right now

journal.cli.client.network.observer.pause = { $verb } is not yet available.

journal.cli.client.network.operation = operation: { $phase }

journal.cli.client.network.option.float = Error: option requires a floating-point value.

journal.cli.client.network.pair.complete = Pair request completed; device should appear in `{ $command }`.

journal.cli.client.network.pair.device = Device: { $device }

journal.cli.client.network.pair.expired = Timed out. Pair code expired.

journal.cli.client.network.pair.fingerprint = { $ca } fingerprint: { $algorithm }{ $fingerprint }

journal.cli.client.network.pair.link = { $token }: { $link }

journal.cli.client.network.pair.paired = Paired: { $label }

journal.cli.client.network.pair.result.date = { $indent }paired_at:{ $indent } { $date }

journal.cli.client.network.pair.result.fingerprint = { $indent }fingerprint: { $fingerprint }

journal.cli.client.network.pair.wait = Waiting for linked system…

journal.cli.client.network.portal.approve = continue to approve → { $url }

journal.cli.client.network.posture.direct = posture: direct

journal.cli.client.network.posture.private = posture: private network

journal.cli.client.network.private.enrolled = enrolled: yes

journal.cli.client.network.private.link.needs.subscription = your private network isn't on yet. your consent is saved. finish turning it on in the services portal, then turn it on here again:

journal.cli.client.network.private.link.setting.up = setting up your private network...

journal.cli.client.network.private.link.subscribing = your private network isn't on yet. your consent is saved. finish turning it on in the services portal within the hour, and your journal picks it up on its own:

journal.cli.client.network.private.state.enabled = state: enabled

journal.cli.client.network.private.state.inconsistent = state: needs repair

journal.cli.client.network.private.state.not.enabled = state: not enabled

journal.cli.client.network.private.state.other = state: { $state }

journal.cli.client.network.private.state.unknown = state: unknown

journal.cli.client.network.private.unenrolled = enrolled: no

journal.cli.client.network.reach.direct = Reach posture: direct

journal.cli.client.network.reach.private = Reach posture: private network

journal.cli.client.network.relay.url = relay URL: { $url }

journal.cli.client.network.role.invalid = invalid role; expected one of: { $phone }, { $observer }

journal.cli.client.network.status.enrolled = Enrolled:      yes

journal.cli.client.network.status.network.enabled = Private network: enabled

journal.cli.client.network.status.network.inconsistent = Private network: needs repair

journal.cli.client.network.status.network.not.enabled = Private network: not enabled

journal.cli.client.network.status.network.other = Private network: { $state }

journal.cli.client.network.status.network.unknown = Private network: unknown

journal.cli.client.network.status.relay = Relay URL:     { $url }

journal.cli.client.network.status.relay.absent = Relay URL:     None

journal.cli.client.network.status.unenrolled = Enrolled:      no

journal.cli.client.network.time.day = { $quantity ->
    [one] { $quantity } day ago
   *[other] { $quantity } days ago
    }

journal.cli.client.network.time.hour = { $quantity ->
    [one] { $quantity } hour ago
   *[other] { $quantity } hours ago
    }

journal.cli.client.network.time.minute = { $quantity ->
    [one] { $quantity } minute ago
   *[other] { $quantity } minutes ago
    }

journal.cli.client.network.time.month = { $quantity ->
    [one] { $quantity } month ago
   *[other] { $quantity } months ago
    }

journal.cli.client.network.time.second = { $quantity ->
    [one] { $quantity } second ago
   *[other] { $quantity } seconds ago
    }

journal.cli.client.network.time.week = { $quantity ->
    [one] { $quantity } week ago
   *[other] { $quantity } weeks ago
    }

journal.cli.client.network.timeout.integer = Error: option { $flag } requires an integer.

journal.cli.client.network.unpaired = Unpaired.

journal.cli.client.option.required = Error: option { $option } is required.

journal.cli.client.option.unknown = Error: unknown option { $option }.

journal.cli.client.option.value = Error: option { $option } requires an argument.

journal.cli.client.stdin.read = native { $brand } { $input } read failed: { $error }

journal.cli.client.transport.timeout = { $method } { $path } exceeded local { $service } timeout (connect={ $connect }s, read={ $read }s, total={ $total }s)

journal.cli.client.transport.timeout.open = { $method } { $path } exceeded local { $service } timeout (connect={ $connect }s, read=none, total=none)

journal.cli.client.trash.moved = Moved '{ $name }' to the trash.

journal.cli.client.trash.moved.day = Moved '{ $name }' to the trash. It's erased { $day } unless it's recovered.

journal.cli.client.trash.moved.failed = Moved '{ $name }' to the trash. It couldn't be erased now.

journal.cli.client.trash.moved.failed.day = Moved '{ $name }' to the trash. It couldn't be erased now. It's erased { $day } unless it's recovered.

journal.cli.sol.call.activities.day.invalid = Error: invalid day '{ $value }'

journal.cli.sol.call.activities.day.missing = Error: day is required (pass as argument or set { $literal_1 }).

journal.cli.sol.call.activities.empty = No activities found.

journal.cli.sol.call.activities.exists = Error: activity already exists: { $detail }

journal.cli.sol.call.activities.facet.missing = Error: facet is required (pass as argument or set { $literal_1 }).

journal.cli.sol.call.activities.facet.unknown = Error: unknown activity for facet '{ $facet }': { $activity_type }

journal.cli.sol.call.activities.kind.option = Error: { $literal_1 } is required.

journal.cli.sol.call.activities.kind.required = Error: activity is required.

journal.cli.sol.call.activities.participation.array = Error: { $literal_1 } must be an array

journal.cli.sol.call.activities.participation.confidence = Error: { $literal_1 }[{ $index }] '{ $literal_2 }' must be a number

journal.cli.sol.call.activities.participation.context = Error: { $literal_1 }[{ $index }] '{ $literal_2 }' must be a string

journal.cli.sol.call.activities.participation.name = Error: { $literal_1 }[{ $index }] requires a non-empty string '{ $literal_2 }'

journal.cli.sol.call.activities.participation.object = Error: { $literal_1 }[{ $index }] must be an object

journal.cli.sol.call.activities.participation.role = Error: { $literal_1 }[{ $index }] has invalid role '{ $role }' (must be one of ['{ $literal_2 }', '{ $literal_3 }', '{ $literal_4 }', '{ $literal_5 }'])

journal.cli.sol.call.activities.participation.source = Error: { $literal_1 }[{ $index }] has invalid source '{ $source }' (must be one of ['{ $literal_2 }', '{ $literal_3 }', '{ $literal_4 }', '{ $literal_5 }', '{ $literal_6 }'])

journal.cli.sol.call.activities.patch.disallowed = Error: disallowed update fields: { $fields }

journal.cli.sol.call.activities.patch.empty = Error: update payload must include at least one mutable field.

journal.cli.sol.call.activities.range.options = Error: { $literal_1 } is incompatible with { $literal_2 }/{ $literal_3 }.

journal.cli.sol.call.activities.range.reversed = Error: { $literal_1 } ({ $end_day }) must not be before { $literal_2 } ({ $start_day })

journal.cli.sol.call.activities.segment.invalid = Error: invalid { $literal_1 } '{ $segment }' (expected { $literal_2 })

journal.cli.sol.call.activities.source.create = Error: { $literal_1 } must be '{ $literal_2 }'.

journal.cli.sol.call.activities.source.list = Error: { $literal_1 } must be '{ $literal_2 }' or '{ $literal_3 }'.

journal.cli.sol.call.activities.span.absent = activity not found: { $span_id }

journal.cli.sol.call.activities.span.missing = Error: missing argument '{ $literal_1 }'.

journal.cli.sol.call.activities.stdin.json = Error: invalid { $literal_1 } on stdin: { $error }

journal.cli.sol.call.activities.stdin.object = Error: expected { $literal_1 } object on stdin.

journal.cli.sol.call.activities.title.option = Error: { $literal_1 } is required.

journal.cli.sol.call.activities.title.required = Error: title is required.

journal.cli.sol.call.body.coverage = coverage: { $start } to { $end }

journal.cli.sol.call.body.day = day: { $day }

journal.cli.sol.call.body.days = days: { $days }

journal.cli.sol.call.body.entries = entries: { $count }

journal.cli.sol.call.body.imports = imports: { $imported }

journal.cli.sol.call.body.range = window: { $start } to { $end }

journal.cli.sol.call.body.sources = sources: { $sources }

journal.cli.sol.call.body.unreadable = the response from the body app couldn't be read.

journal.cli.sol.call.facets.accepted = Accepted facet candidate '{ $name_key }' as '{ $facet }'.

journal.cli.sol.call.facets.action = { $action } result for '{ $name_key }': { $result }

journal.cli.sol.call.facets.already.accepted = Facet candidate '{ $name_key }' already accepted.

journal.cli.sol.call.facets.already.dismissed = Facet candidate '{ $name_key }' already dismissed.

journal.cli.sol.call.facets.candidate = { $name }  [{ $status }]  count={ $count }  last={ $day }

journal.cli.sol.call.facets.dismissed = Dismissed facet candidate '{ $name_key }'.

journal.cli.sol.call.facets.empty = No facet candidates found.

journal.cli.sol.call.thinking.attestation.expires = { $field_key }: { $value }

journal.cli.sol.call.thinking.attestation.observed = { $field_key }: { $value }

journal.cli.sol.call.thinking.attestation.reason = { $field_key }: { $value }

journal.cli.sol.call.thinking.attestation.state = { $field_key }: { $value }

journal.cli.sol.call.thinking.continuing = operation continues server-side; { $literal_1 } shows its progress.

journal.cli.sol.call.thinking.enabled = { $field_key }: { $value }

journal.cli.sol.call.thinking.ended = operation ended without enabling confidential processing; check { $literal_1 }.

journal.cli.sol.call.thinking.lane = active lane: { $lane }

journal.cli.sol.call.thinking.next = next: { $literal_1 }

journal.cli.sol.call.thinking.not.ready = not ready

journal.cli.sol.call.thinking.operation = operation: { $operation_phase }

journal.cli.sol.call.thinking.portal = continue in browser { $arrow } { $url }

journal.cli.sol.call.thinking.provenance = { $field_key }: { $value }

journal.cli.sol.call.thinking.ready = ready

journal.cli.sol.call.thinking.subscribe = { $field_key }: { $url }

journal.cli.sol.call.transcripts.actionable = { $marker } actionable: { $literal_1 }

journal.cli.sol.call.transcripts.day.totals = { $day }  transcripts:{ $n_transcripts } percepts:{ $n_percepts }

journal.cli.sol.call.transcripts.full.raw = Error: Cannot use { $literal_1 } and { $literal_2 } together.

journal.cli.sol.call.transcripts.heading.percepts = Percepts:

journal.cli.sol.call.transcripts.heading.transcripts = Transcripts:

journal.cli.sol.call.transcripts.length.integer = Error: option { $literal_1 } requires an integer.

journal.cli.sol.call.transcripts.max.integer = Error: option { $literal_1 } requires an integer.

journal.cli.sol.call.transcripts.month.empty = No data for { $month }.

journal.cli.sol.call.transcripts.none = (none)

journal.cli.sol.call.transcripts.pending = { $count ->
    [one] { $indent }{ $start } - { $end } ({ $display_count } segment pending at { $starts })
   *[other] { $indent }{ $start } - { $end } ({ $display_count } segments pending at { $starts })
    }

journal.cli.sol.call.transcripts.range.flags = Error: Cannot mix { $literal_1 }, { $literal_2 }, and { $literal_3 }/{ $literal_4 }.

journal.cli.sol.call.transcripts.segments.empty = No segments.

journal.cli.sol.call.transcripts.source.flags = Error: Cannot mix { $literal_1 }/{ $literal_2 } with individual source flags.

journal.cli.sol.call.transcripts.speakers.heading = Speakers for { $day }/{ $stream }/{ $segment }:

journal.cli.sol.call.transcripts.start.invalid = Error: start time must be { $literal_1 }.

journal.cli.sol.call.transcripts.tag.owner = { $marker } not actionable: { $literal_1 }

journal.cli.sol.call.transcripts.total.days = { $count ->
    [one] Total: { $display_count } day with data
   *[other] Total: { $display_count } days with data
    }

journal.cli.sol.call.transcripts.truncated = [truncated: { $total } bytes total, { $literal_1 } { $maximum }]

journal.cli.sol.call.transcripts.unknown.voice = unknown voice

journal.cli.sol.call.typer.error = Error

journal.cli.sol.call.typer.missing.argument = Missing argument '{ $name }'.

journal.cli.sol.call.typer.missing.option = Missing option '{ $option }'.

journal.cli.sol.call.typer.try = Try '{ $command } { $literal_1 }' for help.

journal.cli.sol.entities.accept.merge.candidate.accept.result.for = accept result for { $source_slug } -> { $target_slug }: { $status }

journal.cli.sol.entities.accept.merge.candidate.accepted.merge.candidate = Accepted merge candidate: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.accept.merge.candidate.merge.candidate.already.accepted = Merge candidate already accepted: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.accept.merge.candidate.merge.is.permanent = Merge { $merge_id } is permanent.

journal.cli.sol.entities.aka.added.alias.to = Added alias '{ $aka_value }' to '{ $resolved_name }'.

journal.cli.sol.entities.aka.alias.already.exists.for = Alias '{ $aka_value }' already exists for '{ $resolved_name }'.

journal.cli.sol.entities.aka.alias.is.the.first.word = Alias '{ $aka_value }' is the first word of '{ $resolved_name }' (skipped).

journal.cli.sol.entities.ambiguities.error.status.must.be.open = Error: { $option_status } must be open or resolved.

journal.cli.sol.entities.ambiguities.from = { $spacing_2 }from: { $sources }

journal.cli.sol.entities.ambiguities.no.entity.ambiguities.found = No entity ambiguities found.

journal.cli.sol.entities.ambiguities.scope.tier = { $ambiguity_id }{ $spacing_2 }{ $name }{ $spacing_2 }[{ $status }]{ $spacing_2 }scope={ $scope }{ $spacing_2 }tier={ $tier }

journal.cli.sol.entities.ambiguities.score = { $spacing_2 }- { $candidate } score={ $score }

journal.cli.sol.entities.attach.entity.already.attached = Entity '{ $entity }' already attached.

journal.cli.sol.entities.attach.entity.attached = Entity '{ $entity }' attached.

journal.cli.sol.entities.busy = that entity couldn't be updated right now because it was busy. try again in a moment.

journal.cli.sol.entities.delete.approval = Deleting moves the entity and its facet links to the trash; { $option_erase } erases it for good. Once the owner approves, run it again with { $option_yes }.

journal.cli.sol.entities.delete.entity.erased = Erased '{ $entity_id }'.

journal.cli.sol.entities.delete.entity.error.wasn.t.deleted = Error: '{ $entity_id }' wasn't deleted: { $reason }.

journal.cli.sol.entities.delete.failed.without.reason.error.wasn.t.deleted.it = Error: '{ $entity_id }' wasn't deleted: it wasn't deleted.

journal.cli.sol.entities.detect.entity.detected.for = Entity '{ $entity_name }' detected for { $day }.

journal.cli.sol.entities.dismiss.merge.candidate.dismiss.result.for = dismiss result for { $source_slug } -> { $target_slug }: { $status }

journal.cli.sol.entities.dismiss.merge.candidate.dismissed.merge.candidate = Dismissed merge candidate: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.dismiss.merge.candidate.merge.candidate.already.dismissed = Merge candidate already dismissed: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.edge.resolution.error.error.entity.not.found = Error: Entity '{ $query }' not found.

journal.cli.sol.entities.edge.resolution.error.error.entity.not.found.did = Error: Entity '{ $query }' not found. Did you mean: { $labels }

journal.cli.sol.entities.edge.resolution.without.query.empty.error.entity.entity.not.found = Error: Entity 'entity' not found.

journal.cli.sol.entities.edge.resolution.without.query.suggestions.error.entity.entity.not.found = Error: Entity 'entity' not found. Did you mean: { $labels }

journal.cli.sol.entities.edges.unavailable = Connections aren't available right now.

journal.cli.sol.entities.entity.error.error.entity.is.hidden = Error: Entity '{ $name }' is hidden.

journal.cli.sol.entities.entity.error.error.entity.not.found = Error: Entity '{ $name }' not found.

journal.cli.sol.entities.entity.error.error.invalid.entity.type = Error: Invalid entity type '{ $entity_type }'.

journal.cli.sol.entities.entity.versions.merge = { $spacing_2 }merge={ $merge_id } ({ $description })

journal.cli.sol.entities.entity.versions.no.identity.history.for = No identity history for { $entity }.

journal.cli.sol.entities.entity.versions.unknown.no.identity.history.for.entity = No identity history for entity.

journal.cli.sol.entities.error.without.name.hidden.error.entity.entity.is.hidden = Error: Entity 'entity' is hidden.

journal.cli.sol.entities.evidence.default.day.unknown.day = unknown-day

journal.cli.sol.entities.evidence.kind.missing.unknown = unknown

journal.cli.sol.entities.history.all.evidence.rows.for = { $quantity ->
    [one] { $total } evidence row for { $entity_id } <-> { $peer }:
   *[other] { $total } evidence rows for { $entity_id } <-> { $peer }:
    }

journal.cli.sol.entities.history.no.recorded.connection.history.between = No recorded connection history between { $entity_id } and { $peer }.

journal.cli.sol.entities.history.no.visible.rows = { $quantity ->
    [one] { $total } evidence row for { $entity_id } <-> { $peer } (showing 0):
   *[other] { $total } evidence rows for { $entity_id } <-> { $peer } (showing 0):
    }

journal.cli.sol.entities.history.range.evidence.rows.for.showing = { $quantity ->
    [one] { $total } evidence row for { $entity_id } <-> { $peer } (showing { $first }-{ $last }):
   *[other] { $total } evidence rows for { $entity_id } <-> { $peer } (showing { $first }-{ $last }):
    }

journal.cli.sol.entities.list.attached.attached.entities = { $quantity ->
    [one] { $count } attached entity:
   *[other] { $count } attached entities:
    }

journal.cli.sol.entities.list.detected.detected.for.entities = { $quantity ->
    [one] { $count } entity detected for { $day }:
   *[other] { $count } detected for { $day } entities:
    }

journal.cli.sol.entities.list.hidden.hidden = { $spacing_2 }- { $name } hidden ({ $entity_type }): { $description }

journal.cli.sol.entities.list.no.entities.found = No entities found.

journal.cli.sol.entities.list.visible.row = { $spacing_2 }- { $name } ({ $entity_type }): { $description }

journal.cli.sol.entities.merge.approval = A merge can't be undone. Preview it without { $option_commit }, then, once the owner approves, run it again with { $option_commit } { $option_yes }.

journal.cli.sol.entities.merge.candidates.facet.detections.needs.last = { $source } -> { $target }{ $spacing_2 }[{ $status }]{ $spacing_2 }facet={ $facet }{ $spacing_2 }detections={ $detections }{ $spacing_2 }needs={ $needs }{ $spacing_2 }last={ $last }

journal.cli.sol.entities.merge.candidates.no.merge.candidates.found = No merge candidates found.

journal.cli.sol.entities.merge.failure.default.error.operation.failed = Error: operation failed

journal.cli.sol.entities.merge.preview.aliases.added = { $spacing_2 }aliases added: { $aliases }

journal.cli.sol.entities.merge.preview.emails.added = { $spacing_2 }emails added: { $emails }

journal.cli.sol.entities.merge.preview.facet.links.moved.merged = { $spacing_2 }facet links: { $moved } moved, { $merged } merged

journal.cli.sol.entities.merge.preview.merge.preview = Merge preview:

journal.cli.sol.entities.merge.preview.observations.moved = { $spacing_2 }observations moved: { $observations }

journal.cli.sol.entities.merge.preview.segment.update.errors = { $spacing_2 }segment update errors: { $errors }

journal.cli.sol.entities.merge.preview.speaker.labels.updated.labels.corrections = { $quantity ->
    [one] { $corrections_quantity ->
        [one] { $spacing_2 }speaker labels updated: { $labels } label, { $corrections } correction
       *[other] { $spacing_2 }speaker labels updated: { $labels } label, { $corrections } corrections
    }
   *[other] { $corrections_quantity ->
        [one] { $spacing_2 }speaker labels updated: { $labels } labels, { $corrections } correction
       *[other] { $spacing_2 }speaker labels updated: { $labels } labels, { $corrections } corrections
    }
    }

journal.cli.sol.entities.merge.preview.this.preview.covers.alias.and = { $spacing_2 }This preview covers alias and email additions; the merge may also change facet links, notes, speaker labels, and voice samples.

journal.cli.sol.entities.merge.preview.voice.samples.moved.added.total = { $spacing_2 }voice samples moved: { $added } added, { $total } total

journal.cli.sol.entities.move.entity.error.facet.from.does.not = Error: Facet '{ $from_facet }' ({ $option_from }) does not exist.

journal.cli.sol.entities.move.entity.error.facet.to.does.not = Error: Facet '{ $to_facet }' ({ $option_to }) does not exist.

journal.cli.sol.entities.move.entity.moved.entity.from.to = Moved entity '{ $entity_name }' from '{ $from_facet }' to '{ $to_facet }'.

journal.cli.sol.entities.neighbor.count = count={ $count }

journal.cli.sol.entities.neighbor.kinds = kinds={ $kinds }

journal.cli.sol.entities.neighbor.score = score={ $score }

journal.cli.sol.entities.network.all.recorded.connections.for = { $quantity ->
    [one] { $total } recorded connection for { $entity_id }:
   *[other] { $total } recorded connections for { $entity_id }:
    }

journal.cli.sol.entities.network.directed.both.directed.out.in = directed=out:{ $out_count }, in:{ $in_count }

journal.cli.sol.entities.network.directed.in.directed.in = directed=in:{ $in_count }

journal.cli.sol.entities.network.directed.out.directed.out = directed=out:{ $out_count }

journal.cli.sol.entities.network.limited.recorded.connections.for.showing = { $quantity ->
    [one] { $total } recorded connection for { $entity_id } (showing { $shown }):
   *[other] { $total } recorded connections for { $entity_id } (showing { $shown }):
    }

journal.cli.sol.entities.network.no.recorded.connections.for = No recorded connections for { $entity_id }.

journal.cli.sol.entities.network.seen.range.seen = seen={ $first }..{ $last }

journal.cli.sol.entities.network.seen.seen = seen={ $day }

journal.cli.sol.entities.observation.error.couldn.t.read.the.moments = couldn't read the moments saved for '{ $name }': line { $line_str } of the file isn't valid. nothing was changed.

journal.cli.sol.entities.observations.newest.moments.for.in.newest.first = { $quantity ->
    [one] { $total } moment for '{ $name }' in '{ $facet }', newest first:
   *[other] { $total } moments for '{ $name }' in '{ $facet }', newest first:
    }

journal.cli.sol.entities.observations.nothing.with.yet = nothing with { $resolved_name } yet.

journal.cli.sol.entities.observations.oldest.moments.for.in.oldest.first = { $quantity ->
    [one] { $total } moment for '{ $name }' in '{ $facet }', oldest first:
   *[other] { $total } moments for '{ $name }' in '{ $facet }', oldest first:
    }

journal.cli.sol.entities.observations.partial.most.recent.showing.the.most.recent.of = showing the { $shown } most recent of { $total } moments for '{ $name }'. add { $option_all } to see every one.

journal.cli.sol.entities.observations.partial.oldest.showing.the.oldest.of.moments = showing the { $shown } oldest of { $total } moments for '{ $name }'. add { $option_all } to see every one.

journal.cli.sol.entities.observe.added.added.moments.for.in = { $quantity ->
    [one] added. { $count } moment for '{ $name }' in '{ $facet }'.
   *[other] added. { $count } moments for '{ $name }' in '{ $facet }'.
    }

journal.cli.sol.entities.observe.your.journal.already.holds.that = your journal already holds that moment for '{ $resolved_name }'. nothing was added.

journal.cli.sol.entities.overview.all.network.overview.edges.across.entities = { $quantity ->
    [one] { $entities_quantity ->
        [one] Network overview: { $edges } edge across { $entities } entity:
       *[other] Network overview: { $edges } edge across { $entities } entities:
    }
   *[other] { $entities_quantity ->
        [one] Network overview: { $edges } edges across { $entities } entity:
       *[other] Network overview: { $edges } edges across { $entities } entities:
    }
    }

journal.cli.sol.entities.overview.kinds = Kinds: { $kinds }

journal.cli.sol.entities.overview.limited.network.overview.edges.across.entities = { $quantity ->
    [one] { $entities_quantity ->
        [one] Network overview: { $edges } edge across { $entities } entity (showing { $shown }):
       *[other] Network overview: { $edges } edge across { $entities } entities (showing { $shown }):
    }
   *[other] { $entities_quantity ->
        [one] Network overview: { $edges } edges across { $entities } entity (showing { $shown }):
       *[other] Network overview: { $edges } edges across { $entities } entities (showing { $shown }):
    }
    }

journal.cli.sol.entities.overview.no.recorded.connections.in.the = No recorded connections in the edge index.

journal.cli.sol.entities.overview.type = type={ $entity_type }

journal.cli.sol.entities.record.merge.candidate.merge.candidate.recorded = merge candidate recorded: { $source } -> { $target }

journal.cli.sol.entities.record.merge.candidate.merge.candidate.updated.status = merge candidate updated: { $source } -> { $target } (status: { $status })

journal.cli.sol.entities.resolve.ambiguity.group.error.missing.required.option.member = Error: missing required option { $option_member_id }

journal.cli.sol.entities.resolve.ambiguity.group.error.missing.required.option.revision = Error: missing required option { $option_revision }

journal.cli.sol.entities.resolve.ambiguity.group.refusing.to.resolve.this.ambiguity = Refusing to resolve this ambiguity group without { $option_yes }.

journal.cli.sol.entities.resolve.ambiguity.group.resolved.ambiguities.to = { $quantity ->
    [one] Resolved { $count } ambiguity to { $entity_id }.
   *[other] Resolved { $count } ambiguities to { $entity_id }.
    }

journal.cli.sol.entities.resolve.ambiguity.refusing.to.resolve.this.ambiguity = Refusing to resolve this ambiguity without { $option_yes }.

journal.cli.sol.entities.resolve.ambiguity.resolved.to.at = Resolved { $ambiguity_id } to { $entity_id } at { $resolved_at }.

journal.cli.sol.entities.resolve.day.error.day.is.required.pass = Error: day is required (pass as argument or set { $literal_sol_day }).

journal.cli.sol.entities.resolve.error.error.entity.is.hidden = Error: Entity '{ $blocked_name }' is hidden.

journal.cli.sol.entities.resolve.error.error.entity.not.found.did = Error: Entity '{ $entity }' not found. Did you mean: { $names }

journal.cli.sol.entities.resolve.error.error.entity.not.found.in = Error: Entity '{ $entity }' not found in facet '{ $facet }'.

journal.cli.sol.entities.resolve.facet.error.facet.is.required.pass = Error: facet is required (pass as argument or set { $literal_sol_facet }).

journal.cli.sol.entities.restore.review.error.missing.required.option.ambiguity = Error: missing required option { $option_ambiguity_id }

journal.cli.sol.entities.restore.review.error.missing.required.option.facet = Error: missing required option { $option_facet }

journal.cli.sol.entities.restore.review.error.missing.required.option.source = Error: missing required option { $option_source_slug }

journal.cli.sol.entities.restore.review.error.missing.required.option.target = Error: missing required option { $option_target_slug }

journal.cli.sol.entities.restore.review.error.target.must.be.ambiguity = Error: { $literal_target } must be ambiguity or merge_candidate

journal.cli.sol.entities.restore.review.refusing.to.restore.review.item = Refusing to restore review item without { $option_yes }.

journal.cli.sol.entities.restore.review.restored.review.item = Restored review item { $target }.

journal.cli.sol.entities.restore.version.refusing.to.restore.this.identity = Refusing to restore this identity version without { $option_yes }.

journal.cli.sol.entities.restore.version.restored.from.new.history.version = Restored { $entity_id } from { $version_id }; new history version { $version }.

journal.cli.sol.entities.review.review.items.ambiguity.card.s = { $quantity ->
    [one] { $ambiguities_quantity ->
        [one] { $candidates_quantity ->
            [one] Review items: { $cards } ambiguity card, { $ambiguities } set-aside ambiguity, { $candidates } set-aside merge candidate.
           *[other] Review items: { $cards } ambiguity card, { $ambiguities } set-aside ambiguity, { $candidates } set-aside merge candidate(s).
        }
       *[other] { $candidates_quantity ->
            [one] Review items: { $cards } ambiguity card, { $ambiguities } set-aside ambiguity(ies), { $candidates } set-aside merge candidate.
           *[other] Review items: { $cards } ambiguity card, { $ambiguities } set-aside ambiguity(ies), { $candidates } set-aside merge candidate(s).
        }
    }
   *[other] { $ambiguities_quantity ->
        [one] { $candidates_quantity ->
            [one] Review items: { $cards } ambiguity card(s), { $ambiguities } set-aside ambiguity, { $candidates } set-aside merge candidate.
           *[other] Review items: { $cards } ambiguity card(s), { $ambiguities } set-aside ambiguity, { $candidates } set-aside merge candidate(s).
        }
       *[other] { $candidates_quantity ->
            [one] Review items: { $cards } ambiguity card(s), { $ambiguities } set-aside ambiguity(ies), { $candidates } set-aside merge candidate.
           *[other] Review items: { $cards } ambiguity card(s), { $ambiguities } set-aside ambiguity(ies), { $candidates } set-aside merge candidate(s).
        }
    }
    }

journal.cli.sol.entities.search.activity.unreadable = Detected entity activity is unreadable. Run `{ $command }`, repair the reported record, and try again.

journal.cli.sol.entities.search.entities = { $quantity ->
    [one] { $count } entity:
   *[other] { $count } entities:
    }

journal.cli.sol.entities.search.facets = { $spacing_4 }facets: { $facets }

journal.cli.sol.entities.search.index.busy = Entity search is unavailable while indexing is in progress. Try again when it finishes.

journal.cli.sol.entities.search.index.stale = The entity search index is stale. Run `{ $command }` and try again.

journal.cli.sol.entities.search.index.unavailable = The entity search index is unavailable. Run `{ $command }` and try again.

journal.cli.sol.entities.sweep.review.completed.entity.review.sweep = Completed entity review sweep.

journal.cli.sol.entities.sweep.review.refusing.to.run.review.sweep = Refusing to run review sweep without { $option_yes }.

journal.cli.sol.entities.update.entity.updated = Entity '{ $resolved_name }' updated.

journal.cli.sol.entities.update.entity.updated.for = Entity '{ $entity }' updated for { $day }.

journal.cli.sol.health.activities.count = { $space_2 }{ $activities_count }: { $value }

journal.cli.sol.health.activities.user.edited = { $space_2 }{ $activities_user_edited }: { $value }

journal.cli.sol.health.activities.with.participation = { $space_2 }{ $activities_with_participation }: { $value }

journal.cli.sol.health.activities.with.story = { $space_2 }{ $activities_with_story }: { $value }

journal.cli.sol.health.argument.unexpected = Error: unexpected argument { $token }.

journal.cli.sol.health.backlog.days = { $days_quantity ->
    [one] { $space_3 }across { $days } day
   *[other] { $space_3 }across { $days } days
    }

journal.cli.sol.health.backlog.days.incomplete = { $days_quantity ->
    [one] { $space_3 }across { $days } day (status incomplete)
   *[other] { $space_3 }across { $days } days (status incomplete)
    }

journal.cli.sol.health.backlog.media.days = { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }{ $media } segment waiting for media processing across { $days } day
   *[other] { $space_2 }{ $media } segment waiting for media processing across { $days } days
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }{ $media } segments waiting for media processing across { $days } day
   *[other] { $space_2 }{ $media } segments waiting for media processing across { $days } days
    }
    }

journal.cli.sol.health.backlog.media.days.incomplete = { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }at least { $media } segment waiting for media processing across { $days } day (status incomplete)
   *[other] { $space_2 }at least { $media } segment waiting for media processing across { $days } days (status incomplete)
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }at least { $media } segments waiting for media processing across { $days } day (status incomplete)
   *[other] { $space_2 }at least { $media } segments waiting for media processing across { $days } days (status incomplete)
    }
    }

journal.cli.sol.health.backlog.thinking.days = { $thinking_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segment awaiting thinking across { $days } day
   *[other] { $space_2 }{ $thinking } segment awaiting thinking across { $days } days
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segments awaiting thinking across { $days } day
   *[other] { $space_2 }{ $thinking } segments awaiting thinking across { $days } days
    }
    }

journal.cli.sol.health.backlog.thinking.days.incomplete = { $thinking_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }at least { $thinking } segment awaiting thinking across { $days } day (status incomplete)
   *[other] { $space_2 }at least { $thinking } segment awaiting thinking across { $days } days (status incomplete)
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }at least { $thinking } segments awaiting thinking across { $days } day (status incomplete)
   *[other] { $space_2 }at least { $thinking } segments awaiting thinking across { $days } days (status incomplete)
    }
    }

journal.cli.sol.health.backlog.thinking.media.days = { $thinking_quantity ->
    [one] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segment awaiting thinking, { $media } segment waiting for media processing across { $days } day
   *[other] { $space_2 }{ $thinking } segment awaiting thinking, { $media } segment waiting for media processing across { $days } days
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segment awaiting thinking, { $media } segments waiting for media processing across { $days } day
   *[other] { $space_2 }{ $thinking } segment awaiting thinking, { $media } segments waiting for media processing across { $days } days
    }
    }
   *[other] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segments awaiting thinking, { $media } segment waiting for media processing across { $days } day
   *[other] { $space_2 }{ $thinking } segments awaiting thinking, { $media } segment waiting for media processing across { $days } days
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segments awaiting thinking, { $media } segments waiting for media processing across { $days } day
   *[other] { $space_2 }{ $thinking } segments awaiting thinking, { $media } segments waiting for media processing across { $days } days
    }
    }
    }

journal.cli.sol.health.backlog.thinking.media.days.incomplete = { $thinking_quantity ->
    [one] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }at least { $thinking } segment awaiting thinking, at least { $media } segment waiting for media processing across { $days } day (status incomplete)
   *[other] { $space_2 }at least { $thinking } segment awaiting thinking, at least { $media } segment waiting for media processing across { $days } days (status incomplete)
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }at least { $thinking } segment awaiting thinking, at least { $media } segments waiting for media processing across { $days } day (status incomplete)
   *[other] { $space_2 }at least { $thinking } segment awaiting thinking, at least { $media } segments waiting for media processing across { $days } days (status incomplete)
    }
    }
   *[other] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }at least { $thinking } segments awaiting thinking, at least { $media } segment waiting for media processing across { $days } day (status incomplete)
   *[other] { $space_2 }at least { $thinking } segments awaiting thinking, at least { $media } segment waiting for media processing across { $days } days (status incomplete)
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }at least { $thinking } segments awaiting thinking, at least { $media } segments waiting for media processing across { $days } day (status incomplete)
   *[other] { $space_2 }at least { $thinking } segments awaiting thinking, at least { $media } segments waiting for media processing across { $days } days (status incomplete)
    }
    }
    }

journal.cli.sol.health.backlog.unavailable = { $space_2 }Segment analysis status unavailable

journal.cli.sol.health.consumer = Consumer Signals

journal.cli.sol.health.coverage.ratio = { $space_2 }{ $coverage_ratio }: { $value }

journal.cli.sol.health.facets.silent.past.day = { $space_2 }{ $facets_silent_24h }: { $value }

journal.cli.sol.health.facets.with.recent.capture = { $space_2 }{ $facets_with_recent_capture }: { $value }

journal.cli.sol.health.hours.total = { $space_2 }{ $hours_total }: { $value }

journal.cli.sol.health.hours.with.capture = { $space_2 }{ $hours_with_capture }: { $value }

journal.cli.sol.health.index = { $space_2 }search_index: { $state } (stale { $stale }, missing { $missing }, orphaned { $orphaned }, failed { $failed })

journal.cli.sol.health.index.activity.at = { $space_2 }{ $index_activity_at }: { $value }

journal.cli.sol.health.intake = Intake

journal.cli.sol.health.last.segment.at = { $space_2 }{ $last_segment_at }: { $value }

journal.cli.sol.health.none = { $space_2 }none

journal.cli.sol.health.notes = Notes

journal.cli.sol.health.profile.entities.total = { $space_2 }{ $profile_entities_total }: { $value }

journal.cli.sol.health.range = Range: { $start } -> { $end }

journal.cli.sol.health.silent = Silent Facet Detail

journal.cli.sol.health.synthesis = Synthesis

journal.cli.sol.health.talent.degraded.outputs.past.day = { $space_2 }{ $talent_degraded_outputs_24h }: { $value }

journal.cli.sol.health.talent.run.failures.past.day = { $space_2 }{ $talent_run_failures_24h }: { $value }

journal.cli.sol.import.already.started = already started{ $line_break }

journal.cli.sol.import.argument.media = the following arguments are required: media

journal.cli.sol.import.argument.unexpected = unexpected argument(s): { $arguments }

journal.cli.sol.import.argument.unknown = unrecognized arguments: { $token }

journal.cli.sol.import.duplicate.entries = { $entries_quantity ->
    [one] { $solstone_import }: already imported on { $date } ({ $entries } entry); skipping
   *[other] { $solstone_import }: already imported on { $date } ({ $entries } entries); skipping
    }

journal.cli.sol.import.duplicate.entries.literal = { $solstone_import }: already imported on { $date } ({ $entries } entries); skipping{ $line_break }

journal.cli.sol.import.duplicate.entries.literal.unknown = { $solstone_import }: already imported on unknown date ({ $entries } entries); skipping{ $line_break }

journal.cli.sol.import.duplicate.entries.unknown = { $entries_quantity ->
    [one] { $solstone_import }: already imported on unknown date ({ $entries } entry); skipping
   *[other] { $solstone_import }: already imported on unknown date ({ $entries } entries); skipping
    }

journal.cli.sol.import.duplicate.imported = { $solstone_import }: already imported on { $date }; skipping{ $line_break }

journal.cli.sol.import.duplicate.imported.unknown = { $solstone_import }: already imported on unknown date; skipping{ $line_break }

journal.cli.sol.import.duplicate.skipping = { $solstone_import }: duplicate import; skipping{ $line_break }

journal.cli.sol.import.duplicate.staged = { $solstone_import }: already staged as { $import_id }; skipping{ $line_break }

journal.cli.sol.import.duplicate.staged.unknown = { $solstone_import }: already staged as unknown; skipping{ $line_break }

journal.cli.sol.import.help = usage: { $solstone_import_h_timestamp_ti }{ $line_break }{ $line_break }Import media through the journal{ $line_break }

journal.cli.sol.import.host.guidance = `{ $auto } { $guidance }` requires the journal host. Use `{ $timestamp }` here or run `{ $solstone_journal_importer }`.

journal.cli.sol.import.host.option = `{ $option }` requires the journal host. Run this on the journal host with `{ $solstone_journal_importer }`.

journal.cli.sol.import.journal.unreachable = { $solstone_import }: couldn't reach the journal. Start it with '{ $solstone_journal_up }' and retry.{ $line_break }

journal.cli.sol.import.notice.failed = could not show import day notice: { $error }

journal.cli.sol.import.queued = queued processing{ $line_break }

journal.cli.sol.import.response.malformed = { $solstone_import }: couldn't read journal response{ $line_break }

journal.cli.sol.import.size.failed = could not show import size: { $error }

journal.cli.sol.import.size.skipped = { $skipped } skipped{ $line_break }

journal.cli.sol.import.size.total = { $recordings_quantity ->
    [one] { $recordings } recording across { $days_quantity ->
    [one] { $days } day
   *[other] { $days } days
    }
   *[other] { $recordings } recordings across { $days_quantity ->
    [one] { $days } day
   *[other] { $days } days
    }
    }

journal.cli.sol.import.stage.failed = { $solstone_import }: failed to stage import: { $error }{ $line_break }

journal.cli.sol.import.staged.failed = { $solstone_import }: staged { $staged_path } but processing was not queued: { $error }{ $line_break }

journal.cli.sol.import.staged.malformed = { $solstone_import }: staged { $staged_path } but processing was not queued: couldn't read journal response{ $line_break }

journal.cli.sol.import.staged.path = staged { $path }{ $line_break }

journal.cli.sol.import.staged.timestamp = timestamp { $timestamp }{ $line_break }

journal.cli.sol.import.staged.unreachable = { $solstone_import }: staged { $staged_path } but processing was not queued: couldn't reach the journal{ $line_break }

journal.cli.sol.import.task.queued = queued processing task { $task_id }{ $line_break }

journal.cli.sol.import.task.started = already started as task { $task_id }{ $line_break }

journal.cli.sol.import.usage.error = usage: { $solstone_import_h_timestamp_ti }{ $line_break }{ $line_break }Import media through the journal{ $line_break }{ $solstone_import }: error: { $error }{ $line_break }

journal.cli.sol.journal.facet.delete.yes = Error: { $yes } is required to delete a facet.

journal.cli.sol.link.argument.expected = argument { $option }: expected one argument

journal.cli.sol.link.argument.unrecognized = unrecognized arguments: { $unknown }

journal.cli.sol.link.bundle.chain.empty = { $chain_pem } contains no certificates

journal.cli.sol.link.bundle.chain.incomplete = { $chain_pem } contains an incomplete certificate

journal.cli.sol.link.bundle.endpoint.ip = { $lan } endpoint missing ip

journal.cli.sol.link.bundle.endpoint.port = { $lan } endpoint has invalid port

journal.cli.sol.link.bundle.endpoints.list = { $peer_json } { $local_endpoints } must be a list

journal.cli.sol.link.bundle.endpoints.objects = { $peer_json } { $local_endpoints } entries must be objects

journal.cli.sol.link.bundle.exists = Credentials path already exists: { $path }. Remove it and rerun if re-pairing.

journal.cli.sol.link.bundle.exists.spent = Credentials path already exists: { $path }. The pairing code is now spent; generate a new one and rerun after removing it.

journal.cli.sol.link.bundle.files.incomplete = credential bundle file set is incomplete

journal.cli.sol.link.bundle.files.missing = missing { $pl } bundle file: { $paths }

journal.cli.sol.link.bundle.home.missing = could not resolve home directory for this device's pairing credentials.

journal.cli.sol.link.bundle.invalid = invalid link bundle for label '{ $label }' at { $path }: { $error }. Run `{ $solstone_link_join }` to pair this device.

journal.cli.sol.link.bundle.multiple = multiple link bundles found: { $labels }. pass { $label } to choose one.

journal.cli.sol.link.bundle.none = no link bundles found under { $path }. run `{ $solstone_link_join }` to pair this device.

journal.cli.sol.link.bundle.path.missing = { $pl } bundle not found: { $path }

journal.cli.sol.link.bundle.peer.invalid = invalid { $peer_json } in { $path }: { $error }

journal.cli.sol.link.confirm.done = your journal's mark is already confirmed. nothing to do.{ $line_break }

journal.cli.sol.link.confirm.error = { $solstone_link_confirm }: error: { $error }{ $line_break }

journal.cli.sol.link.confirm.help = usage: { $solstone_link_confirm_h_label_ }{ $line_break }{ $line_break }Confirm the journal's mark for a link waiting on it.{ $line_break }{ $line_break }options:{ $line_break }{ $h_help }{ $space_5 }show this help message and exit{ $line_break }{ $label_label }{ $space_2 }Link bundle label (defaults to the only paired link){ $line_break }{ $mark_mark }{ $space_4 }the two words of the mark your journal's network app shows.{ $line_break }{ $space_17 }needed when there's no terminal to ask you on.{ $line_break }

journal.cli.sol.link.confirm.success = paired.{ $line_break }

journal.cli.sol.link.confirm.usage.error = usage: { $solstone_link_confirm_h_label_ }{ $line_break }{ $solstone_link_confirm }: error: { $error }{ $line_break }

journal.cli.sol.link.held = waiting for you to confirm your journal's mark. nothing waiting goes into your journal until you do.{ $line_break }when you're ready, run: { $solstone_link_confirm } { $label_1 } { $label }{ $line_break }

journal.cli.sol.link.join.code.invalid = Pair code did not match an accepted form. Use a pair-link like { $https_go_solstone_app_p }... from '{ $solstone_call_link_pair }'.

journal.cli.sol.link.join.code.malformed = Malformed pair-link. Use the full { $https_go_solstone_app_p }... value from the pairing output.

journal.cli.sol.link.join.code.nonlocal = Pair-link points at an address outside the local network this joiner will dial.

journal.cli.sol.link.join.help = usage: { $solstone_link_join_h_home_home }{ $line_break }{ $label_label_mark_mark }{ $line_break }{ $line_break }options:{ $line_break }{ $h_help }{ $space_5 }show this help message and exit{ $line_break }{ $home_home }{ $space_4 }Receiver base { $url }{ $line_break }{ $code_code }{ $space_4 }pair-link { $url }{ $line_break }{ $as_as_role }{ $space_3 }Optional tag to join as{ $line_break }{ $label_label }{ $space_2 }Local credentials label (defaults to this machine's hostname){ $line_break }{ $mark_mark }{ $space_4 }the two words of the mark your journal's network app shows.{ $line_break }{ $space_17 }needed when there's no terminal to ask you on.{ $line_break }

journal.cli.sol.link.join.invalid.role = invalid role; expected one of: { $phone }, { $observer }{ $line_break }

journal.cli.sol.link.join.lock.error = { $solstone_link_join }: error: { $err }{ $line_break }

journal.cli.sol.link.join.paired = paired.{ $line_break }credentials: { $path }{ $line_break }

journal.cli.sol.link.join.pairing.unavailable = Link pairing seam is unavailable.{ $line_break }

journal.cli.sol.link.join.required.code = the following arguments are required: { $code }

journal.cli.sol.link.join.response.large = Pair response { $local_endpoints } is too large.{ $line_break }

journal.cli.sol.link.join.target.host = Pair-link target missing host.

journal.cli.sol.link.join.target.port = Pair-link target missing explicit port.

journal.cli.sol.link.join.usage.error = usage: { $solstone_link_join_h_home_home }{ $line_break }{ $label_label_mark_mark }{ $line_break }{ $solstone_link_join }: error: { $error }{ $line_break }

journal.cli.sol.link.label.character = { $label } may contain only letters, numbers, '-', '_', and '.'

journal.cli.sol.link.label.empty = { $label } must not be empty

journal.cli.sol.link.label.length = { $label } must be 80 characters or fewer

journal.cli.sol.link.label.separator = { $label } must not contain path separators

journal.cli.sol.link.label.start = { $label } must not start with '.'

journal.cli.sol.link.label.traversal = { $label } must not contain '..'

journal.cli.sol.link.mark.ask.identified = does this match your journal? type { $yes } or { $no }:

journal.cli.sol.link.mark.ask.unavailable = type { $continue } to pair anyway, or { $cancel } to stop:

journal.cli.sol.link.mark.confirm.cancel = pairing cancelled. get a fresh pairing link from your journal and run { $solstone_link_join } again when you're ready.{ $line_break }

journal.cli.sol.link.mark.confirm.mismatch = not paired.{ $line_break }you said this mark doesn't match the one your journal shows, so this computer isn't paired. you may have pasted the wrong link, or something isn't right. get a fresh pairing link from your journal and run { $solstone_link_join } again, or email { $support_solstone_app } and we'll help.{ $line_break }

journal.cli.sol.link.mark.confirm.terminal = { $solstone_link_confirm } can't ask you about your journal's mark here. run it in a terminal, or give the mark's two words with { $mark_word_word }.{ $line_break }

journal.cli.sol.link.mark.confirm.unavailable = this computer couldn't work out the journal's mark, so the words you gave can't be matched. waiting for you to confirm your journal's mark. nothing waiting goes into your journal until you do. run { $solstone_link_confirm } in a terminal to decide for yourself.{ $line_break }

journal.cli.sol.link.mark.confirm.wrong = not paired.{ $line_break }the mark words you gave don't match the journal's mark, so this computer isn't paired. check the words, get a fresh pairing link from your journal and run { $solstone_link_join } again.{ $line_break }

journal.cli.sol.link.mark.join.cancel = pairing cancelled. nothing this computer has kept went to that journal through this link. get a fresh pairing link from your journal and run { $solstone_link_join } again when you're ready.{ $line_break }

journal.cli.sol.link.mark.join.mismatch = not paired.{ $line_break }you said this mark doesn't match the one your journal shows, so this computer isn't paired, and nothing it has kept went to that journal through this link. you may have pasted the wrong link, or something isn't right. get a fresh pairing link from your journal and run { $solstone_link_join } again, or email { $support_solstone_app } and we'll help.{ $line_break }

journal.cli.sol.link.mark.join.unverifiable = couldn't verify.{ $line_break }this computer couldn't work out the journal's mark, so the words you gave can't be matched. this computer isn't paired, and nothing it has kept went to that journal through this link. get a fresh pairing link from your journal and run { $solstone_link_join } in a terminal to decide for yourself.{ $line_break }

journal.cli.sol.link.mark.join.wrong = not paired.{ $line_break }the mark words you gave don't match the journal's mark, so this computer isn't paired, and nothing it has kept went to that journal through this link. check the words, get a fresh pairing link from your journal and run { $solstone_link_join } again.{ $line_break }

journal.cli.sol.link.mark.prompt.identified = one more step: check your journal's mark.{ $line_break }{ $line_break }{ $space_2 }your journal's mark: { $spoken }{ $line_break }{ $line_break }your journal shows this same mark in its network app. it should match, exactly.{ $line_break }does this match your journal? type { $yes } or { $no }:{ $line_break }

journal.cli.sol.link.mark.prompt.unavailable = one more step: check your journal's mark.{ $line_break }{ $line_break }{ $space_2 }your journal's mark: unavailable right now{ $line_break }{ $line_break }couldn't verify.{ $line_break }this computer couldn't work out your journal's mark, so there's nothing to compare. { $continue } only if you're sure the link came from your journal.{ $line_break }type { $continue } to pair anyway, or { $cancel } to stop:{ $line_break }

journal.cli.sol.link.mark.terminal.required = { $solstone_link_join } can't ask you about your journal's mark here. run it in a terminal, or give the mark's two words with { $mark_word_word }.{ $line_break }

journal.cli.sol.link.mark.usage = give the two words of your journal's mark, like { $mark_bramble_quokka }.{ $line_break }

journal.cli.sol.link.pair.closed = Pairing failed: the pairing window is closed or expired. Generate a new pair code and retry.

journal.cli.sol.link.pair.enrollment = Relay rejected device enrollment ({ $endpoint } { $http } { $status }).

journal.cli.sol.link.pair.failed = Pairing failed ({ $code }). Generate a new pair code and retry.

journal.cli.sol.link.pair.listener = Could not connect to the pairing listener.

journal.cli.sol.link.pair.rejected = Pairing failed ({ $http } { $status }): the pairing window is closed or the code was already used.

journal.cli.sol.link.relay.abnormal = The relay connection closed abnormally. Retry or use { $direct } on { $lan_vpn }.

journal.cli.sol.link.relay.closed = The relay pairing window is closed. Re-run { $solstone_link_join } from a fresh code.

journal.cli.sol.link.relay.offline = the relay reports your journal is offline. start the journal or use { $direct } on lan/vpn.

journal.cli.sol.link.relay.overflow = The relay is temporarily overloaded. Retry or use { $direct } on { $lan_vpn }.

journal.cli.sol.link.relay.stalled = The relay connection stalled. Retry or use { $direct } on { $lan_vpn }.

journal.cli.sol.link.relay.unauthorized = the relay rejected this device token. re-run { $solstone_link_join } for this device.

journal.cli.sol.link.relay.unknown = the relay does not know this journal. re-run { $solstone_link_join }.

journal.cli.sol.link.relay.unpaid = The relay account is not available. Check relay service/account status or use { $direct }.

journal.cli.sol.link.relay.upgrade = The relay rejected the WebSocket upgrade. Check { $relay_url } and retry.

journal.cli.sol.link.response.attestation = Pair response missing { $home_attestation }

journal.cli.sol.link.response.cert = Pair response missing { $client_cert }

journal.cli.sol.link.response.chain = Pair response missing { $ca_chain }

journal.cli.sol.link.response.endpoints = Pair response { $local_endpoints } must be an array.

journal.cli.sol.link.response.instance = Pair response missing { $instance_id }

journal.cli.sol.link.response.key = Pair response missing generated client key

journal.cli.sol.link.serve.bind.failed = cannot bind { $value_127_0_0_1 }{ $port }: bind failed

journal.cli.sol.link.serve.bind.in.use = cannot bind { $value_127_0_0_1 }{ $port }: address already in use. Another `{ $solstone_link_serve }` or Convey may already be using that port.

journal.cli.sol.link.serve.bridge.failed = Native link bridge setup failed before serving. Retry after reinstalling { $solstone_core }.

journal.cli.sol.link.serve.credentials.invalid = Link credentials are invalid. Run { $solstone_link_join } before { $solstone_link_serve }.

journal.cli.sol.link.serve.forward.direct = forwarding { $value_127_0_0_1 }{ $port } -> { $mark } via direct connection{ $line_break }

journal.cli.sol.link.serve.forward.direct.unnamed = forwarding { $value_127_0_0_1 }{ $port } -> your journal via direct connection{ $line_break }

journal.cli.sol.link.serve.forward.either = forwarding { $value_127_0_0_1 }{ $port } -> { $mark } via direct or relay{ $line_break }

journal.cli.sol.link.serve.forward.either.unnamed = forwarding { $value_127_0_0_1 }{ $port } -> your journal via direct or relay{ $line_break }

journal.cli.sol.link.serve.forward.relay = forwarding { $value_127_0_0_1 }{ $port } -> { $mark } via relay only{ $line_break }

journal.cli.sol.link.serve.forward.relay.unnamed = forwarding { $value_127_0_0_1 }{ $port } -> your journal via relay only{ $line_break }

journal.cli.sol.link.serve.help = usage: { $solstone_link_serve_h_label_la }{ $line_break }{ $relay_url_relay_url_direct_rel }{ $line_break }{ $line_break }options:{ $line_break }{ $h_help }{ $space_12 }show this help message and exit{ $line_break }{ $label_label }{ $space_9 }Link bundle label{ $line_break }{ $port_port }{ $space_11 }Loopback port to serve on (default: 5015){ $line_break }{ $relay_url_relay_url }{ $line_break }{ $space_24 }Override the relay { $url }{ $line_break }{ $direct }{ $space_14 }Direct only: reach the journal through a direct{ $line_break }{ $space_24 }connection, never the relay. use when your journal is{ $line_break }{ $space_24 }reachable directly (same network/vpn) to avoid relay{ $line_break }{ $space_24 }dependency.{ $line_break }{ $relay_only }{ $space_10 }Relay only: reach the journal through the relay,{ $line_break }{ $space_24 }never a direct connection. use when direct connections{ $line_break }{ $space_24 }must not be attempted, even if your journal is reachable{ $line_break }{ $space_24 }locally.{ $line_break }

journal.cli.sol.link.serve.mark.unnamed = this link bundle has no journal id, so it can't be named. re-run { $solstone_link_join }.{ $line_break }

journal.cli.sol.link.serve.policy.conflict = argument { $direct }: not allowed with argument { $relay_only }

journal.cli.sol.link.serve.port.invalid = argument { $port }: invalid int value: '{ $value }'

journal.cli.sol.link.serve.port.range = { $port } must be between 1 and 65535

journal.cli.sol.link.serve.runtime.unavailable = Native link runtime is unavailable. Reinstall { $solstone_core } and retry.

journal.cli.sol.link.serve.unavailable = Link serve seam is unavailable.{ $line_break }

journal.cli.sol.link.serve.usage.error = usage: { $solstone_link_serve_h_label_la }{ $line_break }{ $relay_url_relay_url_direct_rel }{ $line_break }{ $solstone_link_serve }: error: { $error }{ $line_break }

journal.cli.sol.link.serve.warning = warning: development use on trusted systems only.{ $line_break }this bridge gives local callers the paired owner's full journal { $api } authority.{ $line_break }run it only where you trust all software running on this system.

journal.cli.sol.link.sighting.certainty = { $line_break }{ $line_break }this is less certain than a direct connection.

journal.cli.sol.link.sighting.claim.unshown = what answered:{ $space_2 }claimed identity could not be shown (claimed, not verified)

journal.cli.sol.link.sighting.claimed = what answered:{ $space_2 }{ $spoken } (claimed, not verified)

journal.cli.sol.link.sighting.direct = unknown journal seen at { $addr }:

journal.cli.sol.link.sighting.journal = your journal:{ $space_3 }{ $spoken }

journal.cli.sol.link.sighting.no.identity = what answered:{ $space_2 }no identity presented

journal.cli.sol.link.sighting.relay = unknown journal seen through the relay:

journal.cli.sol.link.sighting.reset = { $line_break }{ $line_break }if you recently reset your journal, pair this device again.

journal.cli.sol.link.status.error = { $solstone_link_status }: error: { $error }{ $line_break }

journal.cli.sol.link.status.help = usage: { $solstone_link_status_h_label_l }{ $line_break }{ $line_break }Show link status and observed remote journal version.{ $line_break }{ $line_break }options:{ $line_break }{ $h_help }{ $space_5 }show this help message and exit{ $line_break }{ $label_label }{ $space_2 }Link bundle label (defaults to the only paired link){ $line_break }

journal.cli.sol.link.status.label = Label: { $label }{ $line_break }

journal.cli.sol.link.status.live = Status: { $state }{ $line_break }

journal.cli.sol.link.status.persist.warning = Warning: connection changes may not have been saved. keep your credentials; the previous state may still be on disk.{ $line_break }

journal.cli.sol.link.status.stopped = Status: stopped{ $line_break }

journal.cli.sol.link.status.usage.error = usage: { $solstone_link_status_h_label_l }{ $line_break }{ $solstone_link_status }: error: { $error }{ $line_break }

journal.cli.sol.link.status.version.fresh = Journal version: { $version }{ $line_break }

journal.cli.sol.link.status.version.last = Journal version: { $version } (last known){ $line_break }

journal.cli.sol.link.status.version.unknown = Journal version: unknown{ $line_break }

journal.cli.sol.link.transport.credential = link credential material is invalid. re-run { $solstone_link_join } for this device.

journal.cli.sol.link.transport.enroll.rejected = Relay enrollment was rejected with { $http } { $status }. Re-run { $solstone_link_join } if the bundle attestation is stale.

journal.cli.sol.link.transport.enrollment = Link credential or relay enrollment failed. Re-run { $solstone_link_join } if retrying does not fix it.

journal.cli.sol.link.transport.framing = { $spl } stream framing failed while serving. Retry; re-pair if it continues.

journal.cli.sol.link.transport.http = The journal response over the link could not be parsed. Update both peers or retry.

journal.cli.sol.link.transport.io = Link transport I/O failed while serving. Check that the journal is reachable on { $lan_vpn } or relay, then retry.

journal.cli.sol.link.transport.json = Relay or bridge { $json } could not be parsed. Check the relay { $url } and retry.

journal.cli.sol.link.transport.no.endpoint = No journal endpoint is available. Re-run { $solstone_link_join } or pass { $relay_url } unless using { $direct } intentionally.

journal.cli.sol.link.transport.not.paired = Link credentials are missing. Run { $solstone_link_join } before { $solstone_link_serve }.

journal.cli.sol.link.transport.offset = Local offset lookup failed. Check the system clock and retry.

journal.cli.sol.link.transport.pair.data = stored pairing data is invalid. re-run { $solstone_link_join } for this device.

journal.cli.sol.link.transport.refresh.rejected = relay token refresh was rejected with { $http } { $status }. re-run { $solstone_link_join } for this device.

journal.cli.sol.link.transport.rejected = The paired journal rejected the link request with { $http } { $status }.

journal.cli.sol.link.transport.tls = Secure link handshake failed. Re-run { $solstone_link_join } if the journal certificate or pairing changed.

journal.cli.sol.link.unpaired.instructions = This device can't connect to your journal with its saved pairing. To pair it again, stop '{ $solstone_link_serve }', remove the folder { $path }, run '{ $solstone_link_join_code_new_pa }', then start '{ $solstone_link_serve }' again.{ $line_break }

journal.cli.sol.moved.navigate = Moved to `{ $solstone_journal_navigate }` — run that instead.{ $line_break }

journal.cli.sol.profile.cadence = Cadence:

journal.cli.sol.profile.description = description: { $value }

journal.cli.sol.profile.detached = detached facets: { $detached_label }

journal.cli.sol.profile.entity.id = entity_id: { $value }

journal.cli.sol.profile.full.interactions = { $space_2 }recent_interactions_count_30d: { $value }

journal.cli.sol.profile.full.interval = { $space_2 }avg_interval_days: { $value }

journal.cli.sol.profile.full.last.seen = { $space_2 }last_seen: { $value }

journal.cli.sol.profile.full.quiet = { $space_2 }gone_quiet_since: { $value }

journal.cli.sol.profile.identity = { $name } · { $kind } · facets={ $facets } · self={ $self_value } · hidden={ $hidden }

journal.cli.sol.profile.interactions = recent_interactions_count_30d: { $value }

journal.cli.sol.profile.interval = avg_interval_days: { $value }

journal.cli.sol.profile.last.seen = last_seen: { $value }

journal.cli.sol.profile.name = name: { $value }

journal.cli.sol.profile.not.found = profile not found: { $name }

journal.cli.sol.profile.quiet = gone_quiet_since: { $value }

journal.cli.sol.profile.type = type: { $value }

journal.cli.sol.speakers.attribute.segment.accumulated.voiceprints = { $line_break }Accumulated voiceprints:

journal.cli.sol.speakers.attribute.segment.by.method = { $line_break }By method:

journal.cli.sol.speakers.attribute.segment.embeddings = { $quantity ->
    [one] { $spacing_2 }{ $entity_id }: { $embeddings } embedding
   *[other] { $spacing_2 }{ $entity_id }: { $embeddings } embeddings
    }

journal.cli.sol.speakers.attribute.segment.resolved = Resolved:{ $spacing_2 }{ $resolved }

journal.cli.sol.speakers.attribute.segment.sentences = Sentences: { $count }

journal.cli.sol.speakers.attribute.segment.unmatched = Unmatched: { $count }

journal.cli.sol.speakers.attribute.segment.wrote = { $line_break }Wrote: { $path }

journal.cli.sol.speakers.backfill.completed.count = Completed count:{ $spacing_11 }{ $count }

journal.cli.sol.speakers.backfill.elapsed.s = Elapsed:{ $spacing_19 }{ $elapsed }s

journal.cli.sol.speakers.backfill.error.count = Error count:{ $spacing_15 }{ $count }

journal.cli.sol.speakers.backfill.errors.and.more = { $spacing_2 }... and { $count } more

journal.cli.sol.speakers.backfill.failure.detail = Failure detail:{ $spacing_12 }{ $detail }

journal.cli.sol.speakers.backfill.failure.stage = Failure stage:{ $spacing_13 }{ $stage }

journal.cli.sol.speakers.backfill.last.seen.entities.seen = Entities seen:{ $spacing_12 }{ $count }

journal.cli.sol.speakers.backfill.last.seen.pending.by.entity = { $line_break }Pending by entity:

journal.cli.sol.speakers.backfill.last.seen.rows.pending = Rows pending:{ $spacing_13 }{ $count }

journal.cli.sol.speakers.backfill.last.seen.rows.written = Rows written:{ $spacing_13 }{ $count }

journal.cli.sol.speakers.backfill.last.seen.speaker.label.files.read = Speaker label files read: { $count }

journal.cli.sol.speakers.backfill.last.seen.voiceprint.rows.scanned = Voiceprint rows scanned:{ $spacing_2 }{ $count }

journal.cli.sol.speakers.backfill.note.backfill.operation.has.immutable = Note: backfill operation { $id } has immutable flags.{ $line_break }To resume with compatible settings, try:{ $line_break }{ $spacing_2 }{ $command } { $option_operation_id } { $id } { $option_commit }{ $line_break }

journal.cli.sol.speakers.backfill.operation.id = Operation ID:{ $spacing_14 }{ $op_id }

journal.cli.sol.speakers.backfill.pending.count = Pending count:{ $spacing_13 }{ $count }

journal.cli.sol.speakers.backfill.protected.skipped = Protected (skipped):{ $spacing_7 }{ $count }

journal.cli.sol.speakers.backfill.selected.for.backfill = Selected for backfill:{ $spacing_5 }{ $count }

journal.cli.sol.speakers.backfill.starting.speaker.attribution.backfill = Starting speaker attribution backfill...

journal.cli.sol.speakers.backfill.status = Status:{ $spacing_20 }{ $status }

journal.cli.sol.speakers.backfill.total.planned.count = Total planned count:{ $spacing_7 }{ $count }

journal.cli.sol.speakers.backfill.total.segments.scanned = Total segments scanned:{ $spacing_4 }{ $count }

journal.cli.sol.speakers.bootstrap.bootstrapping.voiceprints.from.single.speaker = Bootstrapping voiceprints from single-speaker segments...

journal.cli.sol.speakers.bootstrap.stats.embeddings.saved = Embeddings saved: { $count }

journal.cli.sol.speakers.bootstrap.stats.embeddings.skipped.duplicate = Embeddings skipped (duplicate): { $count }

journal.cli.sol.speakers.bootstrap.stats.embeddings.skipped.owner = Embeddings skipped (owner): { $count }

journal.cli.sol.speakers.bootstrap.stats.entities.created = Entities created: { $count }

journal.cli.sol.speakers.bootstrap.stats.segments.scanned = Segments scanned: { $count }

journal.cli.sol.speakers.bootstrap.stats.single.speaker.segments = Single-speaker segments: { $count }

journal.cli.sol.speakers.bootstrap.stats.top.speakers.by.embedding.count = Top speakers by embedding count:

journal.cli.sol.speakers.bootstrap.stats.unique.speakers = Unique speakers: { $count }

journal.cli.sol.speakers.build.from.tags.can.build.from.tags = Can build from tags: { $available }

journal.cli.sol.speakers.build.from.tags.guidance = Guidance: { $guidance }

journal.cli.sol.speakers.build.from.tags.manual.tags = Manual tags: { $tags }

journal.cli.sol.speakers.build.from.tags.next.step = Next step: { $step }

journal.cli.sol.speakers.build.from.tags.observed = Observed: { $observed }

journal.cli.sol.speakers.build.from.tags.owner.centroid.already.confirmed = Owner centroid already confirmed.

journal.cli.sol.speakers.build.from.tags.owner.centroid.confirmed.from.manual = Owner centroid confirmed from manual tags (principal: { $principal }, cluster_size: { $cluster_size })

journal.cli.sol.speakers.build.from.tags.owner.manual.tags.are.not = Owner manual tags are not ready.

journal.cli.sol.speakers.build.from.tags.reason = Reason: { $reason }

journal.cli.sol.speakers.build.from.tags.threshold = Threshold: { $threshold }

journal.cli.sol.speakers.confirm.owner.review.error.owner.voice.candidates.must = error: Owner voice candidates must be confirmed in the browser.{ $line_break }Review and confirm at { $review_entry }

journal.cli.sol.speakers.correct.already.correct = Already correct: { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }

journal.cli.sol.speakers.correct.corrected = Corrected { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }: { $old } -> { $new_speaker }

journal.cli.sol.speakers.correct.preview.with.solstone.call.speakers = Preview with: { $command } { $old_speaker } { $new_speaker }

journal.cli.sol.speakers.correct.propagation.available.statements.in.segments = { $quantity ->
    [one] { $segments_quantity ->
        [one] Propagation available: { $statements } statement in { $segments } segment would change
       *[other] Propagation available: { $statements } statement in { $segments } segments would change
    }
   *[other] { $segments_quantity ->
        [one] Propagation available: { $statements } statements in { $segments } segment would change
       *[other] Propagation available: { $statements } statements in { $segments } segments would change
    }
    }

journal.cli.sol.speakers.correct.propagation.nothing.else.would.change = Propagation: nothing else would change

journal.cli.sol.speakers.correct.unassigned.unassigned = unassigned

journal.cli.sol.speakers.correct.voiceprint.removal = Voiceprint removal: { $outcome }

journal.cli.sol.speakers.day.segments.showing.of.segments.limit = { $quantity ->
    [one] Showing { $shown } of { $total } segment (limit { $limit })
   *[other] Showing { $shown } of { $total } segments (limit { $limit })
    }

journal.cli.sol.speakers.discover.cluster.samples.across.segments = { $quantity ->
    [one] { $segments_quantity ->
        [one] { $spacing_2 }Cluster { $cluster }: { $samples } sample across { $segments } segment
       *[other] { $spacing_2 }Cluster { $cluster }: { $samples } sample across { $segments } segments
    }
   *[other] { $segments_quantity ->
        [one] { $spacing_2 }Cluster { $cluster }: { $samples } samples across { $segments } segment
       *[other] { $spacing_2 }Cluster { $cluster }: { $samples } samples across { $segments } segments
    }
    }

journal.cli.sol.speakers.discover.found.unknown.speaker.cluster.s = { $quantity ->
    [one] Found { $clusters } unknown speaker cluster:{ $line_break }
   *[other] Found { $clusters } unknown speaker cluster(s):{ $line_break }
    }

journal.cli.sol.speakers.discover.no.recurring.unknown.speakers.found = No recurring unknown speakers found.

journal.cli.sol.speakers.discover.sid = { $spacing_4 }- { $day }/{ $stream }/{ $segment } sid={ $sentence_id }: { $text }

journal.cli.sol.speakers.discovery.error.try.again = try again

journal.cli.sol.speakers.discovery.warnings.warning = Warning: { $reason_code }: { $message }

journal.cli.sol.speakers.discovery.warnings.warning.count = Warning: { $reason_code }: { $message } count={ $count }

journal.cli.sol.speakers.errors.errors = { $line_break }Errors ({ $count }):

journal.cli.sol.speakers.follow.backfill.backfill.in.progress.status.completed = Backfill in progress: status={ $status_kind }, completed={ $completed }/{ $total }, pending={ $pending }

journal.cli.sol.speakers.follow.backfill.backfill.not.completed.status.to = { $line_break }Backfill not completed (status: { $status_kind }).{ $line_break }To check status:{ $line_break }{ $spacing_2 }{ $command_1 } { $operation_id }{ $line_break }To resume:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.follow.repair.start.repair.in.progress.segments.repaired = Repair in progress, segments repaired so far: { $count }

journal.cli.sol.speakers.follow.repair.start.repair.running = Repair running: { $operation_id }

journal.cli.sol.speakers.identify.error.inspect.operations.with.solstone.call = Inspect operations with: { $command }

journal.cli.sol.speakers.identify.error.inspect.this.operation.with.solstone = Inspect this operation with: { $command } { $operation_id }

journal.cli.sol.speakers.identify.error.retry.with.the.same.request = Retry with the same { $option_request_id } { $request_id }.

journal.cli.sol.speakers.identify.name.required.usage = Usage: { $command } { $syntax }{ $line_break }Try '{ $help_command }' for help.{ $line_break }{ $box_left } Error { $box_top }{ $line_break }{ $box_edge } Invalid value: name or { $entity_option } is required{ $box_padding }{ $box_edge }{ $line_break }{ $box_bottom }{ $line_break }

journal.cli.sol.speakers.parse.stream.layout.option.error.invalid.value.for.stream = Error: invalid value for { $option_stream_layout }

journal.cli.sol.speakers.presence.candidate.meeting.unvoiced.screen.meeting.days.voice.no = { $spacing_2 }{ $name } ({ $entity }): screen={ $first_count }, meeting_days={ $second_count }, voice=no

journal.cli.sol.speakers.presence.candidate.meeting.voiced.screen.meeting.days.voice.yes = { $spacing_2 }{ $name } ({ $entity }): screen={ $first_count }, meeting_days={ $second_count }, voice=yes

journal.cli.sol.speakers.presence.candidate.mention.unvoiced.setting.speakers.voice.no = { $spacing_2 }{ $name } ({ $entity }): setting={ $first_count }, speakers={ $second_count }, voice=no

journal.cli.sol.speakers.presence.candidate.mention.voiced.setting.speakers.voice.yes = { $spacing_2 }{ $name } ({ $entity }): setting={ $first_count }, speakers={ $second_count }, voice=yes

journal.cli.sol.speakers.presence.cluster.statements.segments.conversations = { $quantity ->
    [one] { $segments_quantity ->
        [one] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } statement, { $segments } segment, { $conversations } conversation
           *[other] Cluster { $cluster }: { $statements } statement, { $segments } segment, { $conversations } conversations
        }
       *[other] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } statement, { $segments } segments, { $conversations } conversation
           *[other] Cluster { $cluster }: { $statements } statement, { $segments } segments, { $conversations } conversations
        }
    }
   *[other] { $segments_quantity ->
        [one] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } statements, { $segments } segment, { $conversations } conversation
           *[other] Cluster { $cluster }: { $statements } statements, { $segments } segment, { $conversations } conversations
        }
       *[other] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } statements, { $segments } segments, { $conversations } conversation
           *[other] Cluster { $cluster }: { $statements } statements, { $segments } segments, { $conversations } conversations
        }
    }
    }

journal.cli.sol.speakers.presence.cluster.was.not.found.run = Cluster { $cluster_id } was not found.{ $line_break }Run '{ $command }' to produce valid cluster ids.{ $line_break }

journal.cli.sol.speakers.presence.co.presence = { $line_break }Co-presence:

journal.cli.sol.speakers.presence.evidence.complete = Evidence: complete

journal.cli.sol.speakers.presence.evidence.gap.s = { $quantity ->
    [one] Evidence: { $gaps } gap
   *[other] Evidence: { $gaps } gap(s)
    }

journal.cli.sol.speakers.presence.mentions = { $line_break }Mentions:

journal.cli.sol.speakers.presence.no.candidate.entities.found = No candidate entities found.

journal.cli.sol.speakers.propagate.applied.applied.statements.in.segments = { $quantity ->
    [one] { $segment_count_quantity ->
        [one] Applied: { $statement_count } statement in { $segment_count } segment
       *[other] Applied: { $statement_count } statement in { $segment_count } segments
    }
   *[other] { $segment_count_quantity ->
        [one] Applied: { $statement_count } statements in { $segment_count } segment
       *[other] Applied: { $statement_count } statements in { $segment_count } segments
    }
    }

journal.cli.sol.speakers.propagate.errors = { $line_break }Errors:

journal.cli.sol.speakers.propagate.nothing.else.would.change = Nothing else would change.

journal.cli.sol.speakers.propagate.reverse.with.solstone.call.commit = Reverse with: { $command } { $operation } { $old_speaker } { $new_speaker } { $option_commit }

journal.cli.sol.speakers.propagate.would.change.would.change.statements.in.segments = { $quantity ->
    [one] { $segment_count_quantity ->
        [one] Would change: { $statement_count } statement in { $segment_count } segment
       *[other] Would change: { $statement_count } statement in { $segment_count } segments
    }
   *[other] { $segment_count_quantity ->
        [one] Would change: { $statement_count } statements in { $segment_count } segment
       *[other] Would change: { $statement_count } statements in { $segment_count } segments
    }
    }

journal.cli.sol.speakers.rebuild.owner.guidance = Guidance: { $step }

journal.cli.sol.speakers.rebuild.owner.next.step = Next step: { $next_step }

journal.cli.sol.speakers.rebuild.owner.override.applied.true = Override applied: true

journal.cli.sol.speakers.rebuild.owner.owner.centroid.rebuild.did.not = Owner centroid rebuild did not write.

journal.cli.sol.speakers.rebuild.owner.owner.centroid.rebuilt.principal.cluster = Owner centroid rebuilt (principal: { $principal }, cluster_size: { $cluster_size })

journal.cli.sol.speakers.rebuild.owner.owner.centroid.unchanged.evidence.already = Owner centroid unchanged; evidence already matches.

journal.cli.sol.speakers.reject.owner.error.version.is.required = Error: { $option_version } is required.

journal.cli.sol.speakers.repair.check.status.advice.to.check.status.solstone.call = { $line_break }To check status:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.repair.dry.run.complete = Complete:{ $spacing_2 }{ $complete }

journal.cli.sol.speakers.repair.dry.run.entities.with.voiceprints.to.remove = { $line_break }Entities with voiceprints to remove: { $entities }

journal.cli.sol.speakers.repair.dry.run.gaps.detected = { $line_break }Gaps detected ({ $gaps }):

journal.cli.sol.speakers.repair.dry.run.mode.dry.run = Mode:{ $spacing_6 }dry-run

journal.cli.sol.speakers.repair.dry.run.rows = { $quantity ->
    [one] { $spacing_2 }- { $id } ({ $rows } row)
   *[other] { $spacing_2 }- { $id } ({ $rows } rows)
    }

journal.cli.sol.speakers.repair.dry.run.segments.to.repair = { $line_break }Segments to repair: { $segments }

journal.cli.sol.speakers.repair.in.progress.status.advice = { $line_break }The repair may still be running in the journal. Don't start another one.{ $line_break }To check status:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.repair.starting.speaker.contamination.repair = Starting speaker contamination repair...

journal.cli.sol.speakers.repair.status.clean = Clean:{ $spacing_5 }{ $clean }

journal.cli.sol.speakers.repair.status.last.failure = Last failure: { $stage } ({ $code })

journal.cli.sol.speakers.repair.status.segments.repaired = Segments repaired: { $count }

journal.cli.sol.speakers.repair.status.status = Status:{ $spacing_4 }{ $status }

journal.cli.sol.speakers.repair.stopped.advice.to.resume.solstone.call.speakers = { $line_break }To resume:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.report.only = REPORT ONLY — pass { $option_commit } to persist.{ $line_break }

journal.cli.sol.speakers.resolve.names.ambiguous = { $line_break }Ambiguous ({ $count }):

journal.cli.sol.speakers.resolve.names.auto.merged = { $line_break }Auto-merged ({ $count }):

journal.cli.sol.speakers.resolve.names.entities.with.voiceprints = Entities with voiceprints: { $count }

journal.cli.sol.speakers.resolve.names.high.similarity.pairs = High-similarity pairs: { $count }

journal.cli.sol.speakers.resolve.names.merging.name.variants.can.t = Merging name variants can't be undone. Review the report without { $option_commit }, then, once the owner approves, run it again with { $option_commit } { $option_yes }.

journal.cli.sol.speakers.resolve.names.pairs.compared = Pairs compared: { $count }

journal.cli.sol.speakers.resolve.names.resolving.speaker.name.variants = Resolving speaker name variants...

journal.cli.sol.speakers.seed.from.imports.seeding.voiceprints.from.import.segments = Seeding voiceprints from import segments...

journal.cli.sol.speakers.seed.from.imports.segments.with.speakers = Segments with speakers: { $count }

journal.cli.sol.speakers.seed.from.imports.speakers.by.embedding.count = Speakers by embedding count:

journal.cli.sol.speakers.seed.from.imports.unmatched.speakers = { $line_break }Unmatched speakers ({ $count }):

journal.cli.sol.speakers.sentences.sentences.for = Sentences for { $day }/{ $stream }/{ $segment }/{ $source }:

journal.cli.sol.speakers.tag.owner.owner.sentence.already.tagged = Owner sentence already tagged: { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }

journal.cli.sol.speakers.tag.owner.tagged.owner.sentence = Tagged owner sentence: { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }

journal.cli.sol.speakers.wipe.couldn.t.erase.now.it = couldn't erase now; it waits in the trash

journal.cli.sol.speakers.wipe.erase.needs.commit = { $option_erase } needs { $option_commit }

journal.cli.sol.speakers.wipe.erased.erased = erased

journal.cli.sol.speakers.wipe.files.b = { $quantity ->
    [one] { $label }: { $files } file ({ $bytes } B)
   *[other] { $label }: { $files } files ({ $bytes } B)
    }

journal.cli.sol.speakers.wipe.in.the.trash = in the trash

journal.cli.sol.speakers.wipe.in.the.trash.erases = in the trash; erases { $day }

journal.cli.sol.speakers.wipe.total.files.b = { $quantity ->
    [one] total{ $spacing_14 }: { $files } file ({ $bytes } B)
   *[other] total{ $spacing_14 }: { $files } files ({ $bytes } B)
    }

journal.cli.social.usage = usage: { $syntax }{ $line_break }{ $line_break }share an activity from your journal with people you choose, through a space on your own atproto account, and follow spaces others share with you. your journal keeps what they share apart from your own memories. development builds only.

journal.cli.social.signin.open = to sign in, open this address in your browser:{ $line_break }{ $address }

journal.cli.social.signin.done = signed in as { $account }.

journal.cli.social.signin.expired = the sign-in wasn't finished in time. run it again to start over.

journal.cli.social.signin.declined = the sign-in was declined, so nothing changed.

journal.cli.social.signed.out = you're not signed in. start with { $sign_in }

journal.cli.social.status.account = signed in as { $account } on { $host }.

journal.cli.social.status.shares = { $share_quantity ->
    [one] sharing { $shares } activity.
   *[other] sharing { $shares } activities.
}

journal.cli.social.status.spaces = { $space_quantity ->
    [one] following { $spaces } space.
   *[other] following { $spaces } spaces.
}

journal.cli.social.status.memories = { $memory_quantity ->
    [one] holding { $memories } activity shared with you.
   *[other] holding { $memories } activities shared with you.
}

journal.cli.social.share.done = shared "{ $title }". send this space address to the people you shared it with, so their journals can follow it:{ $line_break }{ $space }

journal.cli.social.sync.current = every shared activity is up to date.

journal.cli.social.sync.updated = { $activity_quantity ->
    [one] updated { $activities } shared activity.
   *[other] updated { $activities } shared activities.
}

journal.cli.social.sync.removed = { $activity_quantity ->
    [one] removed { $activities } shared activity that's no longer in your journal.
   *[other] removed { $activities } shared activities that are no longer in your journal.
}

journal.cli.social.unshare.done = stopped sharing that activity, and it's gone from every space you shared it in.

journal.cli.social.unshare.unknown = that activity isn't shared.

journal.cli.social.follow.done = following the space { $sharer } shared with you.

journal.cli.social.receive.none = nothing new from { $sharer }.

journal.cli.social.receive.added = { $activity_quantity ->
    [one] { $sharer } shared { $activities } new activity with you.
   *[other] { $sharer } shared { $activities } new activities with you.
}

journal.cli.social.receive.updated = { $activity_quantity ->
    [one] { $sharer } updated { $activities } activity they shared with you.
   *[other] { $sharer } updated { $activities } activities they shared with you.
}

journal.cli.social.receive.removed = { $activity_quantity ->
    [one] { $sharer } stopped sharing { $activities } activity, so it's gone from your journal.
   *[other] { $sharer } stopped sharing { $activities } activities, so they're gone from your journal.
}

journal.cli.social.receive.nothing = you're not following any spaces yet.

journal.cli.social.failed = couldn't finish: { $detail }

journal.cli.social.receive.failed = couldn't read the space { $sharer } shared with you: { $detail }

journal.cli.social.resolve.done = { $account } is { $did }, hosted at { $host }.

journal.cli.host.arguments.invalid = { $command }: error: invalid arguments

journal.cli.host.backfill.facet.ids.help = usage: { $syntax }{ $line_break }{ $line_break }Backfill missing { $term } identifiers across all facet declarations.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_4 }Write backfilled facet IDs to disk (default is dry run){ $line_break }

journal.cli.host.backfill.facet.ids.solstone.journal.backfill.facet.ids.error = { $command_solstone_journal_backfill_facet_ids }: error: { $error }

journal.cli.host.backfill.facet.ids.usage = usage: { $syntax }{ $line_break }

journal.cli.host.backfill.facets.applied = { $scanned_quantity ->
    [one] { $backfilled_quantity ->
        [one] Examined { $scanned } facet; backfilled an ID for { $backfilled } facet.
        *[other] Examined { $scanned } facet; backfilled IDs for { $backfilled } facets.
        }
    *[other] { $backfilled_quantity ->
        [one] Examined { $scanned } facets; backfilled an ID for { $backfilled } facet.
        *[other] Examined { $scanned } facets; backfilled IDs for { $backfilled } facets.
        }
    }

journal.cli.host.backfill.facets.dry = { $scanned_quantity ->
    [one] { $backfilled_quantity ->
        [one] Dry run: examined { $scanned } facet; { $backfilled } facet requires an ID (pass { $literal_commit } to apply).
        *[other] Dry run: examined { $scanned } facet; { $backfilled } facets require IDs (pass { $literal_commit } to apply).
        }
    *[other] { $backfilled_quantity ->
        [one] Dry run: examined { $scanned } facets; { $backfilled } facet requires an ID (pass { $literal_commit } to apply).
        *[other] Dry run: examined { $scanned } facets; { $backfilled } facets require IDs (pass { $literal_commit } to apply).
        }
    }

journal.cli.host.check.help = usage: { $syntax }{ $line_break }{ $line_break }Readiness verdict for bundled local journal models.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_5 }{ $spaces_6 }emit the readiness verdict as { $term } for agents{ $line_break }

journal.cli.host.check.usage = usage: { $syntax }{ $line_break }

journal.cli.host.config.admit.rewrite.identity.admission = solstone journal config: identity admission refused: { $error }

journal.cli.host.config.admit.rewrite.identity.owner = solstone journal config: identity owner refused: { $error }

journal.cli.host.config.admit.rewrite.identity.root = solstone journal config: identity root refused: { $error }

journal.cli.host.config.admit.rewrite.path = solstone journal config: journal path refused: { $error }

journal.cli.host.config.corrupt.settings.file.at.path.couldn = your settings file at { $path } couldn't be read. your settings were not changed. repair the file or restore { $literal_1 } from a backup, then try again.

journal.cli.host.config.cross.filesystem.cannot.move.across.filesystems.filesystem = solstone journal config: refused: cannot move across filesystems (filesystem ids { $source_device } and { $target_device }); stop your journal with '{ $literal_1 }', copy the journal folder to the new path, use { $literal_3 }, then start it again with '{ $literal_2 }'

journal.cli.host.config.decide.move.requires.not.active.target = solstone journal config: refused: { $literal_3 } requires a not active target; current is { $current_state } and target is { $target_state }; valid flags: { $literal_1 }, { $literal_2 }

journal.cli.host.config.execute.already.set.path = solstone journal config: journal already set to { $path }

journal.cli.host.config.execute.before.mutation = solstone journal config: refused before mutation: { $error }

journal.cli.host.config.execute.warning.force.bypasses.confirmation.and = solstone journal config: warning: { $literal_1 } bypasses confirmation and target activity checks

journal.cli.host.config.existing.target.move.target.already.exists.path = solstone journal config: refused: move target already exists: { $path }

journal.cli.host.config.help = usage: { $syntax }{ $line_break }{ $spaces_7 }{ $syntax_4 }{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_10 }show the configured journal path and source{ $line_break }{ $spaces_4 }{ $subcommand_10 }{ $spaces_2 }change the journal path used by this installation{ $line_break }

journal.cli.host.config.identity.root.from.current.executable = solstone journal config: { $error }

journal.cli.host.config.journal.path.contains.shell.active.character = solstone journal config: refused: journal path contains shell-active character: { $path }

journal.cli.host.config.journal.path.source.tree.fallback.path = solstone journal config: refused: { $path } is the source-tree fallback path but this is not a source checkout

journal.cli.host.config.maybe.restart.current.service.could.not.restart.service.at = could not restart service at the original journal folder

journal.cli.host.config.missing.current.move.source.does.not.exist = solstone journal config: refused: move source does not exist: { $path }

journal.cli.host.config.missing.parent.move.target.parent.does.not = solstone journal config: refused: move target parent does not exist: { $directory }

journal.cli.host.config.plan.action.action = action:  { $action }

journal.cli.host.config.plan.current.left.intact.re.adopt = current journal is left intact. to re-adopt it later: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.plan.current.path.state = current: { $path } ({ $state })

journal.cli.host.config.plan.dry.run.yes.nothing.will = dry-run: yes; nothing will be changed

journal.cli.host.config.plan.filesystem.different.devices = filesystem: different devices

journal.cli.host.config.plan.filesystem.same.device = filesystem: same device

journal.cli.host.config.plan.move = move

journal.cli.host.config.plan.plan.summary = { $literal_1 } - plan summary

journal.cli.host.config.plan.proceed = proceed

journal.cli.host.config.plan.re.run.with.yes.proceed = re-run with { $literal_1 } to proceed

journal.cli.host.config.plan.switch = switch

journal.cli.host.config.plan.target.path.state = target:  { $path } ({ $state })

journal.cli.host.config.preflight.coordinates.saved.location.not.recognized.or = the saved journal location is not recognized or could not be read; settings were not changed

journal.cli.host.config.preflight.coordinates.settings.could.not.be.read = your settings could not be read; settings were not changed: { $error }

journal.cli.host.config.prepare.folder.relocation.requires.managed.posix = journal-folder relocation requires a managed { $literal_1 } installation

journal.cli.host.config.read.wrapper.targets.cannot.read.path.e = solstone journal config: refused: cannot read { $path }: { $e }

journal.cli.host.config.read.wrapper.targets.location.changed.while.this.command = journal location changed while this command waited; try the folder change again

journal.cli.host.config.refusal.current.current.state.and.target = solstone journal config: refused: current is { $current_state } and target is { $target_state }; valid flags: { $flags }

journal.cli.host.config.rewrite.preflighted.with.path.not.utf8 = journal path is not { $literal_1 }

journal.cli.host.config.run.move.could.not.stop.service.before = solstone journal config: could not stop service before move ({ $error })

journal.cli.host.config.run.move.could.not.stop.service.before.stop.service.before.move = solstone journal config: could not stop service before move

journal.cli.host.config.run.move.move.failed.e = solstone journal config: move failed: { $e }

journal.cli.host.config.run.move.moved.wrapper.updated.service.restarted = journal moved; wrapper updated; service restarted.

journal.cli.host.config.run.move.remains.at.target.could.not = journal remains at { $target }; could not finish saving the new location: { $e }; retry: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.move.remains.at.target.retry.path = journal remains at { $target }; retry: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.move.service.installed.but.not.running = service installed but not running; journal moved; wrapper updated.

journal.cli.host.config.run.move.service.not.installed.moved.wrapper = service not installed; journal moved; wrapper updated.

journal.cli.host.config.run.move.wrapper.updated.target.but.service = wrapper updated to { $target } but service start failed; restart manually

journal.cli.host.config.run.switch.cannot.create.path.e = solstone journal config: refused: cannot create { $path }: { $e }

journal.cli.host.config.run.switch.cannot.inspect.managed.wrappers = solstone journal config: refused: cannot inspect managed wrappers: { $error }

journal.cli.host.config.run.switch.cannot.lock.managed.wrappers = solstone journal config: refused: cannot lock managed wrappers: { $error }

journal.cli.host.config.run.switch.could.not.finish.saving.new = could not finish saving the new journal location: { $message }; retry: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.switch.could.not.finish.saving.new.journal.path.switch.yes = could not finish saving the new journal location: { $e }; retry: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.switch.could.not.stop.service.before = could not stop service before changing journal folder

journal.cli.host.config.run.switch.now.at.target.could.not = your journal is now at { $target }; could not update its service: { $error }; retry: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.switch.service.installed.but.not.running = service installed but not running; wrapper updated.

journal.cli.host.config.run.switch.service.not.installed.wrapper.updated = service not installed; wrapper updated.

journal.cli.host.config.run.switch.wrapper.rewritten.target.but.service = solstone journal config: wrapper rewritten to { $target } but journal service restart could not run ({ $error }); restart manually

journal.cli.host.config.run.switch.wrapper.rewritten.target.but.service.investigate.and.restart.manually = solstone journal config: wrapper rewritten to { $target } but '{ $literal_1 }' exited { $code }; investigate and restart manually

journal.cli.host.config.run.switch.wrapper.updated.service.restarted = wrapper updated; service restarted.

journal.cli.host.config.run.unsupported.folder.service.action = unsupported journal-folder service action

journal.cli.host.config.service.summary.service.installed.and.running.will = service: installed and running; will stop, move, rewrite wrapper, restart

journal.cli.host.config.service.summary.service.installed.and.running.will.will.rewrite.wrapper.restart = service: installed and running; will rewrite wrapper, restart

journal.cli.host.config.service.summary.service.installed.but.not.running = service: installed but not running; will move and rewrite wrapper

journal.cli.host.config.service.summary.service.installed.but.not.running.running.will.rewrite.wrapper = service: installed but not running; will rewrite wrapper

journal.cli.host.config.service.summary.service.not.installed.will.move = service: not installed; will move and rewrite wrapper

journal.cli.host.config.service.summary.service.not.installed.will.rewrite = service: not installed; will rewrite wrapper

journal.cli.host.config.show.next.next = next: { $next }

journal.cli.host.config.show.source.built.in.default = built-in default ({ $literal_1 })

journal.cli.host.config.show.source.source.tree.fallback = source-tree fallback

journal.cli.host.config.show.source.user.toml = user config ({ $literal_1 })

journal.cli.host.config.state.active = active

journal.cli.host.config.state.not.active = not active

journal.cli.host.config.usage = usage: { $syntax }{ $line_break }{ $spaces_7 }{ $syntax_4 }{ $line_break }

journal.cli.host.config.wrapper.next.move.path.aside.then.run = move { $path } aside, then run '{ $literal_1 }' from the solstone source checkout

journal.cli.host.config.wrapper.refusal.wrapper.not.managed.wrapper.run = solstone journal config: refused: { $wrapper } is not a managed wrapper (run '{ $literal_1 }' from the solstone source checkout to install the wrapper first)

journal.cli.host.contract.build.bundle.browser.schema.bytes.diverged.from = contract: browser schema bytes diverged from { $literal_1 }

journal.cli.host.contract.build.bundle.cannot.read.browser.schema.path = contract: cannot read browser schema { $path }: { $error }

journal.cli.host.contract.build.bundle.invalid.json.schema.path = contract: invalid { $literal_1 } Schema { $path }: { $error }

journal.cli.host.contract.build.bundle.missing.required.schema.sources.sources = contract: missing required schema sources: { $sources }

journal.cli.host.contract.build.bundle.path.duplicate.format.id.format = contract: { $path }: duplicate journal contract format id '{ $format_id }'

journal.cli.host.contract.build.bundle.path.format.id.must.be = contract: { $path }: { $literal_1 } must be a string

journal.cli.host.contract.build.bundle.schema.must.be.json.object = contract: schema must be a { $literal_1 } object: { $path }

journal.cli.host.contract.build.help = usage: { $syntax }{ $line_break }{ $line_break }Build the journal contract bundle.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_15 }Check whether the bundle is current without writing.{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_11 }Contract checkout or installed-package root.{ $line_break }

journal.cli.host.contract.build.usage = usage: { $syntax }{ $line_break }

journal.cli.host.contract.check.help = usage: { $syntax }{ $line_break }{ $line_break }Check the bundle and journal files against their schemas.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_8 }Additional journal root to validate (repeatable).{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_11 }Contract checkout or installed-package root.{ $line_break }

journal.cli.host.contract.check.usage = usage: { $syntax }{ $line_break }

journal.cli.host.contract.classify.breaking.changes.bundle.malformed = solstone journal contract bundle is malformed

journal.cli.host.contract.classify.breaking.changes.format.removed.format = { $format }: removed format

journal.cli.host.contract.classify.breaking.changes.format.removed.key.field.field = { $format }: removed key field '{ $field }'

journal.cli.host.contract.classify.breaking.changes.format.removed.producer.path.path = { $format }: removed producer path '{ $path }'

journal.cli.host.contract.discover.schema.sources.path.missing.meta = contract: { $path }: missing { $contract_meta }

journal.cli.host.contract.fixture.journal.fixture.missing.path = contract: fixture journal missing: { $path }

journal.cli.host.contract.format.errors.invalid.json.schema = invalid { $literal_1 } Schema: { $error }

journal.cli.host.contract.from.root.package.tree.not.found.path = contract: solstone package tree not found: { $path }

journal.cli.host.contract.help = usage: { $syntax }{ $line_break }{ $line_break }Build and validate the journal contract bundle.{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_15 }Build the contract bundle.{ $line_break }{ $spaces_4 }{ $subcommand_8 }{ $spaces_15 }Check the bundle and journal files.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }

journal.cli.host.contract.load.json.invalid.json.in.path = contract: invalid { $literal_1 } in { $path }: { $error }

journal.cli.host.contract.metadata.path.allowed.producers.must.be = contract: { $path }: { $literal_1 } must be a list

journal.cli.host.contract.metadata.path.key.fields.must.be = contract: { $path }: { $literal_1 } must be a list

journal.cli.host.contract.metadata.path.missing.metadata.key = contract: { $path }: missing contract metadata: { $key }

journal.cli.host.contract.resolve.could.not.inspect.current.executable = contract: could not inspect current executable: { $error }

journal.cli.host.contract.resolve.from.executable.could.not.locate.installed.package = contract: could not locate installed solstone package or source checkout from { $path }

journal.cli.host.contract.run.build.cannot.create.bundle.directory = contract: cannot create bundle directory: { $error }

journal.cli.host.contract.run.build.cannot.write.path = contract: cannot write { $path }: { $error }

journal.cli.host.contract.run.build.path.stale.run.build = { $path } is stale; run `{ $literal_1 }`

journal.cli.host.contract.run.build.wrote.path = wrote { $path }

journal.cli.host.contract.run.check.bundle.artifact.missing.path = contract: bundle artifact missing: { $path }

journal.cli.host.contract.run.check.no.covered.files.found.under = contract: no contract-covered files found under { $path }

journal.cli.host.contract.run.check.tree.not.found.path.expected = contract: journal tree not found: { $path } (expected { $expected })

journal.cli.host.contract.usage = usage: { $syntax }{ $line_break }

journal.cli.host.contract.validate.contract.file.unsupported.file.kind.kind = unsupported contract file kind: { $kind }

journal.cli.host.contract.validate.headered.jsonl.headered.jsonl.requires.header.line = { $literal_1 } requires a header line

journal.cli.host.contract.validate.headered.jsonl.invalid.utf8 = invalid { $literal_1 }: { $error }

journal.cli.host.contract.validate.headered.jsonl.line.must.be.json.object = line must be a { $literal_1 } object

journal.cli.host.contract.validate.headered.jsonl.missing.schema.definition = missing schema definition

journal.cli.host.contract.validate.json.invalid.json = invalid { $literal_1 }: { $error }

journal.cli.host.contract.validated = { $quantity ->
    [one] validated { $display_count } contract-covered file under { $root }
   *[other] validated { $display_count } contract-covered files under { $root }
    }

journal.cli.host.contract.walk.four.levels.cannot.read.entry = contract: cannot read journal entry: { $error }

journal.cli.host.contract.walk.schema.files.cannot.read.path = contract: cannot read { $path }: { $error }

journal.cli.host.contract.walk.schema.files.cannot.read.schema.entry = contract: cannot read schema entry: { $error }

journal.cli.host.convey.help = usage: { $syntax }{ $line_break }{ $line_break }{ $term } web interface{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Port to serve on{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_2 }Enable verbose output{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_4 }Enable debug logging{ $line_break }

journal.cli.host.convey.usage = usage: { $syntax }{ $line_break }

journal.cli.host.cortex.help = usage: { $syntax }{ $line_break }{ $line_break }{ $term } { $term_4 } Talent Manager{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_2 }Enable verbose output{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_4 }Enable debug logging{ $line_break }

journal.cli.host.cortex.service.cortex.service.failed = cortex service failed: { $error }

journal.cli.host.cortex.usage = usage: { $syntax }{ $line_break }

journal.cli.host.down.help = usage: { $syntax }{ $line_break }{ $line_break }Stop the journal service.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }

journal.cli.host.facet.candidates.help = usage: { $syntax }{ $line_break }{ $line_break }Record recurring facet review candidates.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }Enable verbose output{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Enable debug logging{ $line_break }

journal.cli.host.facet.candidates.recorded = { $quantity ->
    [one] Recorded/updated { $display_count } facet candidate.
   *[other] Recorded/updated { $display_count } facet candidate(s).
    }

journal.cli.host.facet.candidates.run.facet.candidates = solstone journal facet-candidates: error: { $error }

journal.cli.host.facet.candidates.run.facet.candidates.root.path.not = solstone journal facet-candidates: error: journal root { $path } is not a directory

journal.cli.host.facet.candidates.usage = usage: { $syntax }{ $line_break }

journal.cli.host.grab.help = usage: { $syntax }{ $line_break }{ $line_break }Walk observed screen frames and optionally write frame images.{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_11 }Path tokens: { $path_tokens }{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_6 }Write the selected frame image here ({ $extension }, { $extension_12 }, { $extension_13 }, or{ $line_break }{ $spaces_17 }{ $extension_15 }).{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_8 }Replace an existing output path.{ $line_break }{ $spaces_2 }{ $option_18 }{ $spaces_9 }Emit { $term } instead of table or plain output.{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_2 }Enable verbose output{ $line_break }{ $spaces_2 }{ $option_22 }{ $spaces_4 }Enable debug logging{ $line_break }

journal.cli.host.grab.usage = usage: { $syntax }{ $line_break }

journal.cli.host.health.callosum.clients = { $quantity ->
    [one] Callosum: { $clients } client
   *[other] Callosum: { $clients } clients
    }

journal.cli.host.health.ced.ready = ced.cpp sound-tag engine and model are ready

journal.cli.host.health.ced.unsupported = ced install: unsupported platform { $os }/{ $arch }; skipping ced sound-tag assets

journal.cli.host.health.crash.attempts = { $quantity ->
    [one] { $space_2 }{ $name } { $attempts } restart attempt
   *[other] { $space_2 }{ $name } { $attempts } restart attempts
    }

journal.cli.host.health.crash.reason = { $quantity ->
    [one] { $space_2 }{ $name } { $attempts } restart attempt{ $space_2 }{ $reason }
   *[other] { $space_2 }{ $name } { $attempts } restart attempts{ $space_2 }{ $reason }
    }

journal.cli.host.health.crashed = { $line_break }Crashed:{ $line_break }

journal.cli.host.health.heartbeat.ok = Heartbeat: ok{ $line_break }

journal.cli.host.health.heartbeat.stale = { $line_break }Heartbeat: STALE ({ $stale })

journal.cli.host.health.held = { $space_2 }held { $partition } { $reason }

journal.cli.host.health.help = usage: { $syntax }{ $line_break }{ $line_break }Show the retained supervisor service-health status.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }enable verbose output{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }enable debug output{ $line_break }

journal.cli.host.health.logs.help = usage: { $syntax }{ $line_break }{ $line_break }View operational service logs.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_18 }number of lines to show (default: 5){ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_20 }follow log output{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_10 }show rows at or after { $argument }{ $line_break }{ $spaces_2 }{ $option_13 }{ $spaces_8 }show rows for one service{ $line_break }{ $spaces_2 }{ $option_15 }{ $spaces_8 }show rows matching { $argument_16 }{ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_9 }enable verbose output{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_11 }enable debug output{ $line_break }

journal.cli.host.health.logs.usage = usage: { $syntax }{ $line_break }

journal.cli.host.health.media.absent = Media processor: not reporting

journal.cli.host.health.media.depth.stale = Media processor: { $depth } pending (stale, { $seconds }s since beacon)

journal.cli.host.health.media.idle = Media processor: idle

journal.cli.host.health.media.pending = Media processor: { $depth } pending

journal.cli.host.health.media.stale = Media processor: stale ({ $seconds }s since beacon)

journal.cli.host.health.not.running = Cannot connect: your journal isn't running{ $line_break }

journal.cli.host.health.queue = { $space_2 }queued { $name } { $count }

journal.cli.host.health.queue.hold = { $space_2 }queue-hold { $reason } { $details }

journal.cli.host.health.rfdetr.assets = Object detection is degraded because its RF-DETR assets are unavailable. Screen descriptions will continue. Use `{ $command }` to check or repair the RF-DETR assets.

journal.cli.host.health.rfdetr.package = object detection is unavailable. repair or reinstall your journal.

journal.cli.host.health.rfdetr.ready = rf-detr.cpp object-detection engine and model are ready

journal.cli.host.health.rfdetr.unsupported = rf-detr install: unsupported platform { $os }/{ $arch }; skipping rf-detr object-detection assets

journal.cli.host.health.runtime = Cannot connect: health runtime unavailable: { $message }{ $line_break }

journal.cli.host.health.service.days = { $space_2 }{ $name } pid { $pid }{ $space_2 }uptime { $days }d

journal.cli.host.health.service.days.hours = { $space_2 }{ $name } pid { $pid }{ $space_2 }uptime { $days }d { $hours }h

journal.cli.host.health.service.days.hours.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }uptime { $days }d { $hours }h { $minutes }m

journal.cli.host.health.service.days.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }uptime { $days }d { $minutes }m

journal.cli.host.health.service.hours = { $space_2 }{ $name } pid { $pid }{ $space_2 }uptime { $hours }h

journal.cli.host.health.service.hours.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }uptime { $hours }h { $minutes }m

journal.cli.host.health.service.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }uptime { $minutes }m

journal.cli.host.health.service.seconds = { $space_2 }{ $name } pid { $pid }{ $space_2 }uptime { $seconds }s

journal.cli.host.health.services = Services:{ $line_break }

journal.cli.host.health.socket.encoding = Cannot connect: callosum socket path is not valid UTF-8{ $line_break }

journal.cli.host.health.socket.inspect = Cannot connect: callosum socket is not inspectable: { $reason }{ $line_break }

journal.cli.host.health.socket.missing = Cannot connect: callosum socket not found at { $path }{ $line_break }

journal.cli.host.health.socket.type = path is not a Unix socket

journal.cli.host.health.status.error = Cannot connect: supervisor status error: { $service }: { $reason }{ $line_break }

journal.cli.host.health.status.invalid = Cannot connect: supervisor sent an invalid status payload at { $path }

journal.cli.host.health.task.running = { $space_2 }{ $name } { $duration }s

journal.cli.host.health.task.slow = { $space_2 }{ $name } { $duration }s{ $space_2 }SLOW (cap { $limit }s)

journal.cli.host.health.task.stuck = { $space_2 }{ $name } { $duration }s{ $space_2 }STUCK (cap { $limit }s)

journal.cli.host.health.tasks = Tasks:{ $line_break }

journal.cli.host.health.tasks.empty = Tasks: none{ $line_break }

journal.cli.host.health.timeout = Timed out waiting for supervisor status (10s){ $line_break }

journal.cli.host.health.usage = usage: { $syntax }{ $line_break }

journal.cli.host.heartbeat.error = { $command }: { $detail }

journal.cli.host.heartbeat.help = usage: { $syntax }{ $line_break }{ $line_break }Run deterministic health repair pass{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_5 }{ $spaces_5 }Run full check regardless of recency{ $line_break }

journal.cli.host.heartbeat.oplog = failed to create heartbeat oplog: { $detail }

journal.cli.host.heartbeat.usage = usage: { $syntax }{ $line_break }

journal.cli.host.hosted.service.hosted.service.admission.refused = hosted { $service } service admission refused: { $error }

journal.cli.host.importer.desktop.run.resolved.could.not.show.import.day = could not show import day notice: { $error }{ $line_break }

journal.cli.host.importer.desktop.run.resolved.could.not.show.import.size = could not show import size: { $error }{ $line_break }

journal.cli.host.install.models.coreml.disclosure.parakeet.assets.downloading.parakeet.tdt = parakeet assets: downloading the { $model } { $literal_4 } model ({ $literal_3 }) from { $literal_2 }. see { $literal_1 }.

journal.cli.host.install.models.help = usage: { $syntax }{ $line_break }{ $spaces_30 }{ $syntax_4 }{ $line_break }{ $spaces_30 }{ $syntax_5 }{ $line_break }{ $line_break }install and verify { $term }'s bundled { $argument } models (local { $argument_8 } plus bundled{ $line_break }{ $term_9 }{ $path } assets). default action checks the local { $argument_8 } artifacts and{ $line_break }fetches if missing; { $flag } re-fetches; { $flag_12 } verifies only and exits{ $line_break }nonzero on any problem.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_15 }verify bundled assets and local { $argument_8 } artifacts without{ $line_break }{ $spaces_24 }fetching.{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_15 }ignore readiness and refetch{ $path_20 } local { $argument_8 }{ $line_break }{ $spaces_24 }artifacts.{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_7 }install or verify only bundled models required for{ $line_break }{ $spaces_24 }journal readiness; do not fetch optional providers.{ $line_break }{ $spaces_2 }{ $flag_23 } { $choices }{ $line_break }{ $spaces_24 }journal variant to install or verify. { $literal } honors{ $line_break }{ $spaces_24 }{ $argument_26 } on { $literal_27 }{ $path_28 }, then { $literal }detects.{ $line_break }

journal.cli.host.install.models.install.required.rfdetr.rf.detr.install.unsupported.platform = { $literal_1 } install: unsupported platform { $os }/{ $arch }; skipping { $literal_1 } object-detection assets

journal.cli.host.install.models.install.with.live.progress.parakeet.model.model.name.percent = parakeet model { $model_name }: { $percent }% ({ $received } / { $total } MB)

journal.cli.host.install.models.install.with.live.progress.parakeet.model.model.name.verifying = parakeet model { $model_name }: verifying download

journal.cli.host.install.models.install.with.live.progress.parakeet.setup.download.starting = parakeet setup: download starting

journal.cli.host.install.models.install.with.live.progress.parakeet.setup.downloading.required.files = parakeet setup: downloading required files { $percent }% ({ $received } / { $total } MB)

journal.cli.host.install.models.install.with.live.progress.preparing.parakeet.speech.model.model = preparing parakeet speech model: { $model_name }

journal.cli.host.install.models.parakeet.ready.parakeet.cpp.check.failed.model = { $literal_1 } check failed: model missing at { $path }

journal.cli.host.install.models.parakeet.ready.parakeet.cpp.check.failed.name = { $literal_1 } check failed: { $name } missing at { $path }

journal.cli.host.install.models.parakeet.ready.parakeet.cpp.check.failed.name.not.executable.at.path = { $literal_1 } check failed: { $name } not executable at { $path }

journal.cli.host.install.models.parakeet.ready.parakeet.model.path.missing = parakeet model path missing

journal.cli.host.install.models.parakeet.ready.parakeet.path.missing = parakeet path missing

journal.cli.host.install.models.parakeet.without.download.parakeet.install.unsupported.platform.os = parakeet install: unsupported platform { $os }/{ $arch }; supported: { $literal_1 }, { $literal_2 }

journal.cli.host.install.models.parakeet.without.download.parakeet.speech.recognition.ships.inside = parakeet: speech recognition ships inside the journal app on windows, so there is nothing to download

journal.cli.host.install.models.ready.line.model.ready.path = model ready: { $path }

journal.cli.host.install.models.resolve.variant.invalid.variant.cuda.use.cpu = invalid { $literal_1 }={ $literal_2 }; use { $literal_3 } on { $os }/{ $arch }

journal.cli.host.install.models.resolve.variant.invalid.variant.value.use.cpu = invalid { $literal_1 }='{ $value }'; use { $literal_3 } or { $literal_2 }

journal.cli.host.install.models.resolve.variant.variant.coreml.not.supported.on = variant { $literal_1 } not supported on { $os }

journal.cli.host.install.models.resolve.variant.variant.coreml.not.supported.on.supported.on.os.arch = variant { $literal_1 } not supported on { $os }/{ $arch }

journal.cli.host.install.models.resolve.variant.variant.cpu.not.supported.on = variant { $literal_1 } not supported on { $os }

journal.cli.host.install.models.resolve.variant.variant.cpu.not.supported.on.supported.on.os.arch = variant { $literal_1 } not supported on { $os }/{ $arch }

journal.cli.host.install.models.resolve.variant.variant.cuda.not.supported.on = variant { $literal_1 } not supported on { $os }

journal.cli.host.install.models.resolve.variant.variant.cuda.not.supported.on.supported.on.os.arch = variant { $literal_1 } not supported on { $os }/{ $arch }

journal.cli.host.install.models.rfdetr.bundled.asset.disclosure.rf.detr.assets.verifying.bundled = rf-detr assets: verifying the bundled { $literal_2 } { $version } engine ({ $literal_3 }) and { $literal_4 } nano { $literal_5 } weights ({ $literal_3 }). see { $literal_1 }.

journal.cli.host.install.models.rfdetr.repair.object.detection.unavailable.repair.or = object detection is unavailable. repair or reinstall your journal. { $error }

journal.cli.host.install.models.run.inner.speech.install.already.running.wait = speech install is already running; wait for it to finish

journal.cli.host.install.models.run.inner.with.bundled.asset.verification.failed = bundled asset verification failed: { $error }

journal.cli.host.install.models.run.inner.with.observing.parakeet.install.state.suffix = observing parakeet install: { $state }{ $suffix }

journal.cli.host.install.models.run.inner.with.parakeet.install.already.running.for = parakeet install already running for a different target

journal.cli.host.install.models.run.inner.with.parakeet.install.failed = parakeet install failed

journal.cli.host.install.models.run.inner.with.timed.out.observing.parakeet.install = timed out observing parakeet install

journal.cli.host.install.models.usage = usage: { $syntax }{ $line_break }{ $spaces_30 }{ $syntax_4 }{ $line_break }{ $spaces_30 }{ $syntax_5 }{ $line_break }

journal.cli.host.install.provider.dispatch.message.provider.install.failed = { $provider } install failed

journal.cli.host.install.provider.global.local.model.assets.downloading.llama = local model assets: downloading the { $literal_4 } runtime ({ $literal_7 }; the { $literal_6 } build also carries { $literal_5 }-licensed runtime components) and the model ({ $literal_3 }) from { $literal_2 }. see { $literal_1 }.

journal.cli.host.install.provider.global.local.thinking.downloads.model.and = local thinking downloads the model and vision projector ({ $literal_3 }) from { $literal_2 } when needed. the runtime is included with the journal. see { $literal_1 }.

journal.cli.host.install.provider.global.local.thinking.downloads.model.apache = local thinking downloads the model ({ $literal_3 }) from { $literal_2 }. the { $literal_4 } runtime is included with the journal. see { $literal_1 }.

journal.cli.host.install.provider.global.parakeet.cpp.and.its.speech = { $literal_2 } and its speech model come with the journal on windows, so there's nothing to install. see { $literal_1 }.

journal.cli.host.install.provider.global.parakeet.cpp.can.t.run = { $literal_1 } can't run on a mac. a mac transcribes with its { $literal_2 } model instead.

journal.cli.host.install.provider.global.parakeet.cpp.fetches.two.artifacts = { $literal_4 } fetches two artifacts into this journal's provider cache before it can run, both from { $literal_2 }: the { $literal_3 } server binary ({ $literal_6 }) and the speech model ({ $literal_5 }). see { $literal_1 }.

journal.cli.host.install.provider.help = usage: { $syntax }{ $line_break }{ $line_break }Install or retry a provider runtime.{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_8 }Provider to install: '{ $literal }' or '{ $literal_7 }'.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }

journal.cli.host.install.provider.local.target.sha.local.fingerprint.dispatch.omitted.target = local fingerprint dispatch omitted target_fingerprint_sha256

journal.cli.host.install.provider.observe.existing.with.provider.install.already.running.for = { $provider } install already running for a different target

journal.cli.host.install.provider.observe.existing.with.timed.out.observing.provider.install = timed out observing { $provider } install

journal.cli.host.install.provider.progress.line.observing.provider.install.state.suffix = observing { $provider } install: { $state }{ $suffix }

journal.cli.host.install.provider.run.inner.with.parakeet.already.installed = parakeet already installed

journal.cli.host.install.provider.run.inner.with.unsupported.provider.provider.supported.local = unsupported provider { $provider }; supported: { $literal_1 }

journal.cli.host.install.provider.run.local.inner.with.platform.and.target.sha.local.already.installed = local already installed

journal.cli.host.install.provider.unavailable.status.error.could.not.read.persisted.provider = could not read persisted { $provider } install status: { $error }

journal.cli.host.install.provider.usage = usage: { $syntax }{ $line_break }

journal.cli.host.installation.executable = could not resolve current executable: { $error }

journal.cli.host.installation.executable.parent = current executable has no containing directory

journal.cli.host.installation.recovery = this installation couldn't be verified.{ $line_break }run `{ $setup }` to check it. if setup finishes successfully, try again.{ $line_break }details: { $detail }

journal.cli.host.installation.root = could not resolve installation identity root from { $directory }

journal.cli.host.logs.count = invalid count: { $detail }

journal.cli.host.logs.follow.descriptor = catalog failed for { $path }: oplog descriptor changed while following

journal.cli.host.logs.follow.empty = No log files found.

journal.cli.host.logs.follow.failed = { $operation } failed for { $path }

journal.cli.host.logs.follow.root = catalog failed for { $path }: journal root unavailable

journal.cli.host.logs.follow.source = { $operation } failed for { $path }: { $source }

journal.cli.host.logs.grep.compile = regex pattern could not be compiled

journal.cli.host.logs.grep.invalid = invalid regex pattern at byte offset { $offset }

journal.cli.host.logs.grep.unsupported = unsupported regex feature '{ $family }' at byte offset { $offset }

journal.cli.host.logs.health.error = health logs: { $detail }

journal.cli.host.logs.health.usage = { $command }: error: { $detail }

journal.cli.host.logs.service.empty = === service logs === (no oplog leaves)

journal.cli.host.logs.service.error = service logs: { $detail }

journal.cli.host.logs.service.heading = === service logs ===

journal.cli.host.logs.service.operation = { $operation } failed: { $source }

journal.cli.host.logs.service.unsupported = Error: unsupported platform '{ $platform }'

journal.cli.host.logs.since.invalid = Invalid time: '{ $input }'. Use e.g., { $examples }

journal.cli.host.logs.since.overflow = Python int too large to convert to C int

journal.cli.host.main.windows.service.admission.failed = windows service admission failed: { $error }

journal.cli.host.malformed.jsonl.warning.solstone.observe.utils.invalid.json.at.line.in = { $literal_warning_solstone_observe_utils }:Invalid { $literal_json } at line { $line } in { $path }: { $error }

journal.cli.host.mcp.activity.activity = { $command_solstone_journal_mcp_activity }: { $error }

journal.cli.host.mcp.activity.activity.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_activity } is not compiled into this build

journal.cli.host.mcp.activity.activity.unknown.outcome = { $command_solstone_journal_mcp_activity }: unknown outcome { $value }

journal.cli.host.mcp.activity.activity.unknown.tool = { $command_solstone_journal_mcp_activity }: unknown tool { $value }

journal.cli.host.mcp.activity.bytes = { $space_4 }bytes:{ $space_3 }{ $byte_count }

journal.cli.host.mcp.activity.created = { $space_4 }created: { $created }

journal.cli.host.mcp.activity.help = usage: { $syntax }{ $line_break }{ $spaces_37 }{ $syntax_4 }{ $line_break }{ $spaces_37 }{ $syntax_5 }{ $line_break }{ $line_break }Show what connections asked for and how each request ended.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_8 }only a bearer-token connection, by its label{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_4 }only an { $term } connection, by its client id{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_2 }with { $flag }, which grant, when one client id has several{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_10 }only requests to this tool{ $line_break }{ $spaces_2 }{ $option_18 }{ $spaces_4 }only requests that ended this way{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_2 }from this day{ $line_break }{ $spaces_2 }{ $option_20 }{ $spaces_4 }up to and including this day{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_12 }at most N requests, 1 to 1000 (default: 20){ $line_break }{ $spaces_2 }{ $option_23 }{ $spaces_15 }print { $term_25 }{ $line_break }

journal.cli.host.mcp.activity.no.match = { $examined_quantity ->
    [one] No match in the { $examined } record examined; more may exist further back.
    *[other] No match in the { $examined } records examined; more may exist further back.
    }

journal.cli.host.mcp.activity.no.recorded.mcp.activity.matches = No recorded { $literal_mcp } activity matches.

journal.cli.host.mcp.activity.origin = { $space_4 }origin:{ $space_2 }{ $origin }

journal.cli.host.mcp.activity.raise.limit.or.narrow.day.from.day.to.to.see = Raise { $literal_limit } or narrow { $literal_day_from }/{ $literal_day_to } to see more.

journal.cli.host.mcp.activity.reason = { $space_4 }reason:{ $space_2 }{ $reason }

journal.cli.host.mcp.activity.record = { $space_4 }record:{ $space_2 }{ $day }/{ $segment }

journal.cli.host.mcp.activity.request = { $space_4 }request: { $rendered }

journal.cli.host.mcp.activity.request.arguments.exceeded.the.record.bound.and.were.not.stored = { $space_4 }request: (arguments exceeded the record bound and were not stored)

journal.cli.host.mcp.activity.request.legacy = { $space_4 }request: (schema 1: not recorded)

journal.cli.host.mcp.activity.result = { $count_quantity ->
    [one] { $space_4 }result:{ $space_2 }{ $count } result, digest { $digest }
    *[other] { $space_4 }result:{ $space_2 }{ $count } result(s), digest { $digest }
    }

journal.cli.host.mcp.activity.result.targets = { $count_quantity ->
    [one] { $space_4 }result:{ $space_2 }{ $count } result, digest { $digest }, targets { $targets }
    *[other] { $space_4 }result:{ $space_2 }{ $count } result(s), digest { $digest }, targets { $targets }
    }

journal.cli.host.mcp.activity.result.truncated = { $count_quantity ->
    [one] { $space_4 }result:{ $space_2 }{ $count } result, digest { $digest }, targets { $targets }, ...
    *[other] { $space_4 }result:{ $space_2 }{ $count } result(s), digest { $digest }, targets { $targets }, ...
    }

journal.cli.host.mcp.activity.schema.connection = (schema 1: no connection recorded)

journal.cli.host.mcp.activity.shown.complete = { $shown_quantity ->
    [one] { $shown } record shown of { $examined } examined. { $counts }
    *[other] { $shown } record(s) shown of { $examined } examined. { $counts }
    }

journal.cli.host.mcp.activity.shown.partial = { $shown_quantity ->
    [one] { $shown } record shown of { $examined } examined (more may exist further back). { $counts }
    *[other] { $shown } record(s) shown of { $examined } examined (more may exist further back). { $counts }
    }

journal.cli.host.mcp.activity.unreadable = { $count_quantity ->
    [one] { $count } record in range could not be parsed and is not counted above.
    *[other] { $count } record(s) in range could not be parsed and are not counted above.
    }

journal.cli.host.mcp.help = usage: { $syntax }{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }{ $line_break }subcommands:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_15 }Run the { $term } endpoint{ $line_break }{ $spaces_2 }{ $subcommand_9 }{ $spaces_12 }Run the connection for agents on this computer{ $line_break }{ $spaces_2 }{ $subcommand_10 }{ $spaces_16 }Show { $term } endpoint status{ $line_break }{ $spaces_2 }{ $subcommand_12 }{ $spaces_17 }Manage { $term } bearer tokens{ $line_break }{ $spaces_2 }{ $subcommand_14 }{ $spaces_15 }Manage the agent pairing code{ $line_break }{ $spaces_2 }{ $subcommand_15 }{ $spaces_17 }Manage registered { $term_16 } clients{ $line_break }{ $spaces_2 }{ $subcommand_17 }{ $spaces_12 }Manage connection read permissions{ $line_break }{ $spaces_2 }{ $subcommand_18 }{ $spaces_14 }Show what connections asked for and how each request ended{ $line_break }{ $spaces_2 }{ $subcommand_20 }{ $spaces_17 }Invoke one { $term } read without opening a port{ $line_break }

journal.cli.host.mcp.local.door.help = usage: { $syntax }{ $line_break }{ $line_break }Run the connection for agents on this computer.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }

journal.cli.host.mcp.local.door.local.door.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_local_door } is not compiled into this build

journal.cli.host.mcp.local.door.mcp.local.door.failed = mcp local-door failed: { $class }

journal.cli.host.mcp.oauth.error.detail = { $command_solstone_journal_mcp } { $command } { $operation }: { $error }

journal.cli.host.mcp.oauth.help = usage: { $syntax }{ $line_break }{ $line_break }Manage registered { $term } clients.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }{ $line_break }subcommands:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_8 }List registered { $term } clients{ $line_break }{ $spaces_2 }{ $subcommand_8 }{ $spaces_6 }Revoke a registered { $term } client{ $line_break }

journal.cli.host.mcp.oauth.list.help = usage: { $syntax }{ $line_break }{ $line_break }List registered { $term } clients.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }

journal.cli.host.mcp.oauth.oauth.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_oauth } is not compiled into this build

journal.cli.host.mcp.oauth.oauth.revoke.no.such.oauth.client = { $command_solstone_journal_mcp_oauth_revoke }: no such { $literal_oauth } client

journal.cli.host.mcp.oauth.revoke.help = usage: { $syntax }{ $line_break }{ $line_break }Revoke a registered { $term } client.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_13 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_2 }the client to revoke{ $line_break }

journal.cli.host.mcp.oauth.revoked.oauth.client = Revoked { $literal_oauth } client { $client_id }.

journal.cli.host.mcp.pairing.at.expires.at = Expires at { $expires_at }.

journal.cli.host.mcp.pairing.at.it.works.only.at.your.own.domain = It works only at your own domain.

journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.this.computer = It works only for an agent on this computer.

journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.this.computer.to = It works only for an agent on this computer. To connect an agent another way, use { $literal_door } lan, { $literal_solstone_me } or hostname.

journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.your.network = It works only for an agent on your network.

journal.cli.host.mcp.pairing.at.it.works.only.through.solstone.me = It works only through { $literal_solstone_me }.

journal.cli.host.mcp.pairing.at.pairing.generate.no.hostname.is.set.up.for.this.journal = { $command_solstone_journal_mcp_pairing_generate }: no hostname is set up for this journal. set one up in the agents app first.

journal.cli.host.mcp.pairing.at.pairing.generate.this.journal.s.config.could.not.be.read = { $command_solstone_journal_mcp_pairing_generate }: this journal's config could not be read.

journal.cli.host.mcp.pairing.at.pairing.revoke.no.active.pairing.code.to.revoke = { $command_solstone_journal_mcp_pairing_revoke }: no active pairing code to revoke

journal.cli.host.mcp.pairing.at.revoked.the.active.pairing.code = Revoked the active pairing code.

journal.cli.host.mcp.pairing.at.save.this.now.it.will.not.be.shown.again.and = Save this now. It will not be shown again and cannot be recovered:

journal.cli.host.mcp.pairing.generate.help = usage: { $syntax }{ $line_break }{ $line_break }Generate a pairing code for an agent, valid for 10 minutes and one use. It is shown once.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }{ $spaces_2 }{ $flag } { $choices }{ $line_break }{ $spaces_24 }where the code works (default: { $literal }, an agent on this computer){ $line_break }

journal.cli.host.mcp.pairing.generated = Generated a pairing code, valid for 10 minutes and one use.

journal.cli.host.mcp.pairing.help = usage: { $syntax }{ $line_break }{ $line_break }Manage the agent pairing code.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }{ $line_break }subcommands:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_4 }Generate a pairing code{ $line_break }{ $spaces_2 }{ $subcommand_7 }{ $spaces_6 }Revoke the current pairing code{ $line_break }

journal.cli.host.mcp.pairing.pairing.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_pairing } is not compiled into this build

journal.cli.host.mcp.pairing.revoke.help = usage: { $syntax }{ $line_break }{ $line_break }Revoke the current pairing code.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }

journal.cli.host.mcp.permission.categories = Categories: { $categories }

journal.cli.host.mcp.permission.clear.help = usage: { $syntax }{ $line_break }{ $line_break }Clear one connection's read permission.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_8 }a bearer-token connection, by its label{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_4 }an { $term } connection, by its client id{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_2 }with { $flag }, which grant, when one client id has several{ $line_break }

journal.cli.host.mcp.permission.cleared.permission.for.connection = Cleared permission for connection { $connection }.

journal.cli.host.mcp.permission.connection = Connection: { $connection }

journal.cli.host.mcp.permission.evaluation = Evaluation: { $evaluation }

journal.cli.host.mcp.permission.generation = Generation: { $generation }

journal.cli.host.mcp.permission.help = usage: { $syntax }{ $line_break }{ $line_break }Manage connection read permissions.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }{ $line_break }subcommands:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_8 }Show read permissions, for every connection or one{ $line_break }{ $spaces_2 }{ $subcommand_7 }{ $spaces_9 }Set what one connection may read{ $line_break }{ $spaces_2 }{ $subcommand_9 }{ $spaces_7 }Clear one connection's read permission{ $line_break }

journal.cli.host.mcp.permission.list.facets = { $connection }{ $tab }generation: { $generation }{ $tab }evaluation: { $evaluation }{ $tab }read: facets ({ $facets })

journal.cli.host.mcp.permission.list.none = { $connection }{ $tab }generation: { $generation }{ $tab }evaluation: { $evaluation }{ $tab }no read permission

journal.cli.host.mcp.permission.list.whole = { $connection }{ $tab }generation: { $generation }{ $tab }evaluation: { $evaluation }{ $tab }read: { $literal_whole_journal }

journal.cli.host.mcp.permission.no.mcp.connection.permissions.configured = No { $literal_mcp } connection permissions configured.

journal.cli.host.mcp.permission.no.permission.was.configured.for.connection = No permission was configured for connection { $connection }.

journal.cli.host.mcp.permission.permission.clear = { $command_solstone_journal_mcp_permission_clear }: { $error }

journal.cli.host.mcp.permission.permission.management.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_permission } management is not compiled into this build

journal.cli.host.mcp.permission.permission.no.permission.configured = Permission: no permission configured

journal.cli.host.mcp.permission.permission.set = { $command_solstone_journal_mcp_permission_set }: { $error }

journal.cli.host.mcp.permission.permission.set.choose.a.category.from = { $command_solstone_journal_mcp_permission_set }: choose a category from { $categories }

journal.cli.host.mcp.permission.permission.set.choose.places.precision = { $command_solstone_journal_mcp_permission_set }: choose a places detail level from { $levels }

journal.cli.host.mcp.permission.permission.set.places.precision.requires.places = { $command_solstone_journal_mcp_permission_set }: { $flag } requires { $category_flag }

journal.cli.host.mcp.permission.permission.show = { $command_solstone_journal_mcp_permission_show }: { $error }

journal.cli.host.mcp.permission.places.precision = Places detail: { $precision }

journal.cli.host.mcp.permission.read.no.read.permission.configured = Read:{ $space_7 }no read permission configured

journal.cli.host.mcp.permission.scope.facets = Read scope: facets ({ $facets })

journal.cli.host.mcp.permission.scope.whole = Read scope: { $literal_whole_journal }

journal.cli.host.mcp.permission.set.help = usage: { $syntax }{ $line_break }{ $spaces_43 }{ $syntax_4 }{ $line_break }{ $spaces_43 }{ $syntax_5 }{ $line_break }{ $line_break }Set what one connection may read.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_8 }a bearer-token connection, by its label{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_4 }an { $term } connection, by its client id{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_2 }with { $flag }, which grant, when one client id has several{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_2 }a kind this connection may read (repeat for each kind){ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_8 }only this facet, by name; repeat for more (default: the whole journal){ $line_break }

journal.cli.host.mcp.permission.set.help.places.precision = { $spaces_2 }{ $option }{ $spaces_2 }location detail when { $flag } is set: { $values } (default: { $default }){ $line_break }

journal.cli.host.mcp.permission.set.mcp.read.permission.for.connection.generation = Set { $literal_mcp } read permission for connection { $connection } (generation { $generation }).

journal.cli.host.mcp.permission.show.help = usage: { $syntax }{ $line_break }{ $line_break }Show read permissions, for every connection or for one.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_8 }a bearer-token connection, by its label{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_4 }an { $term } connection, by its client id{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_2 }with { $flag }, which grant, when one client id has several{ $line_break }

journal.cli.host.mcp.probe.help = usage: { $syntax }{ $line_break }{ $spaces_34 }{ $flag } { $argument } { $flag_6 } { $term }{ $line_break }{ $line_break }Invoke one { $term_8 } read as a connection, without opening a port.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_12 }{ $spaces_8 }a bearer-token connection, by its label{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_4 }an { $term_16 } connection, by its client id{ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_2 }with { $flag_18 }, which grant, when one client id has several{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_10 }the tool to call{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_5 }the tool's arguments, as a { $term } object{ $line_break }

journal.cli.host.mcp.service.help = usage: { $syntax }{ $line_break }{ $line_break }Run the { $term } endpoint.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }

journal.cli.host.mcp.service.mcp.service.failed = mcp service failed: { $class }

journal.cli.host.mcp.service.service.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_service } is not compiled into this build

journal.cli.host.mcp.status.capability.compiled.in.true = capability compiled in: true

journal.cli.host.mcp.status.capability.disabled = capability: disabled

journal.cli.host.mcp.status.capability.enabled = capability: enabled

journal.cli.host.mcp.status.capability.error = capability: error

journal.cli.host.mcp.status.help = usage: { $syntax }{ $line_break }{ $line_break }Show { $term } endpoint status.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }

journal.cli.host.mcp.status.lan.disabled = lan door: off at port { $port }

journal.cli.host.mcp.status.lan.enabled = lan door: enabled at port { $port }

journal.cli.host.mcp.status.local.disabled = local door: not enabled at { $resource }

journal.cli.host.mcp.status.local.enabled = local door: enabled at { $resource }

journal.cli.host.mcp.status.mcp.capability.config.status.only.this.does.not.report.service = { $literal_mcp } capability/config status only; this does not report service liveness.

journal.cli.host.mcp.status.status.could.not.read.bearer.token.ledger = { $command_solstone_journal_mcp_status }: could not read bearer-token ledger: { $error }

journal.cli.host.mcp.status.status.could.not.read.mcp.capability.configuration = { $command_solstone_journal_mcp_status }: could not read { $literal_mcp } capability configuration: { $error }

journal.cli.host.mcp.status.status.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_status } is not compiled into this build

journal.cli.host.mcp.status.status.mcp.capability.configuration.is.invalid = { $command_solstone_journal_mcp_status }: { $literal_mcp } capability configuration is invalid: { $error }

journal.cli.host.mcp.status.token.count = token count: { $count }

journal.cli.host.mcp.status.token.unavailable = token count: unavailable

journal.cli.host.mcp.token.create.help = usage: { $syntax }{ $line_break }{ $line_break }Create a bearer token for a connection. The token is shown once.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }a name for the connection the token is for{ $line_break }

journal.cli.host.mcp.token.created.bearer.token.for.label = Created bearer token for label { $label }.

journal.cli.host.mcp.token.grant.before.the.agent.connects.an.agent.that.connected.first = Grant before the agent connects. An agent that connected first keeps its old tool list until it reconnects.

journal.cli.host.mcp.token.help = usage: { $syntax }{ $line_break }{ $line_break }Manage { $term } bearer tokens.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }{ $line_break }subcommands:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_6 }Create a bearer token for a connection{ $line_break }{ $spaces_2 }{ $subcommand_8 }{ $spaces_8 }List tokens{ $line_break }{ $spaces_2 }{ $subcommand_10 }{ $spaces_6 }Revoke a token{ $line_break }

journal.cli.host.mcp.token.list.help = usage: { $syntax }{ $line_break }{ $line_break }List each token's label and when it was created.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }

journal.cli.host.mcp.token.revoke.help = usage: { $syntax }{ $line_break }{ $line_break }Revoke the bearer token for a label.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }the label of the token to revoke{ $line_break }

journal.cli.host.mcp.token.revoked.bearer.token.for.label = Revoked bearer token for label { $label }.

journal.cli.host.mcp.token.save.this.token.now.it.will.not.be.shown.again = Save this token now. It will not be shown again and cannot be recovered:

journal.cli.host.mcp.token.token.management.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_token } management is not compiled into this build

journal.cli.host.mcp.token.until.you.grant.it.read.access.an.agent.using.this = Until you grant it read access, an agent using this token sees only { $literal_save_memory } and { $literal_recall_memory }. To let it read your whole journal (add { $literal_facet } NAME to allow only that facet):

journal.cli.host.mcp.usage = usage: { $syntax }{ $line_break }

journal.cli.host.navigate.error.is.no.longer.supported.put.facet.selection.in.the.destination.url.for = { $command }: error: { $option } is no longer supported. Put facet selection in the destination { $term }; for example, { $path }

journal.cli.host.navigate.help = usage: { $syntax }{ $line_break }{ $line_break }Navigate the browser to a path.{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_18 }{ $term } path to navigate to.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }

journal.cli.host.navigate.print.navigation.navigate.path = Navigate: { $path }

journal.cli.host.navigate.run.navigate.callosum.socket.unavailable.path = solstone journal navigate: error: { $literal_1 } socket unavailable: { $path }

journal.cli.host.navigate.run.navigate.failed.encode.callosum.request = solstone journal navigate: error: failed to encode { $literal_1 } request: { $error }

journal.cli.host.navigate.usage = usage: { $syntax }{ $line_break }

journal.cli.host.open.help = usage: { $syntax }{ $line_break }{ $line_break }open your journal in your browser with a one-time link.{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_10 }page to open (default: /){ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_4 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_3 }print the one-time link instead of opening a browser{ $line_break }

journal.cli.host.open.mint.could.not.make.link = your journal could not make a link: { $error }

journal.cli.host.open.mint.could.not.make.link.try = your journal could not make a link. try again.

journal.cli.host.open.mint.response.from.could.not.be = the response from your journal could not be read. try again.

journal.cli.host.open.run.browser.could.not.be.opened = a browser could not be opened here. run this command again with { $literal_1 }, and open the link it prints within a minute.

journal.cli.host.open.run.open.it.from.another.computer = to open it from another computer, first forward the journal's ports over { $literal_1 }, and keep that terminal open:

journal.cli.host.open.run.open.url.path = open your journal: { $url }{ $path }

journal.cli.host.open.socket.linux.down.isn.t.running.for.this = your journal isn't running for this account on this computer, so there is nothing to open. start it with { $literal_1 }.

journal.cli.host.open.socket.linux.old.running.older.than.this.command = your running journal is older than this command. restart it with { $literal_1 }, then { $literal_2 }, and try again.

journal.cli.host.open.socket.mac.down.isn.t.running.for.this = your journal isn't running for this account on this computer, so there is nothing to open. open the journal app to start it.

journal.cli.host.open.socket.mac.old.running.older.than.this.command = your running journal is older than this command. quit the journal app, open it again, and try again.

journal.cli.host.open.socket.windows.down.isn.t.running.for.this = your journal isn't running for this account on this computer, so there is nothing to open. start it from the journal app, and try again.

journal.cli.host.open.socket.windows.old.running.older.than.this.command = your running journal is older than this command. restart it from the journal app, and try again.

journal.cli.host.open.usage = usage: { $syntax }{ $line_break }

journal.cli.host.parser.convey.argument.journal.expected.one.argument = { $command }: error: argument { $flag }: expected one argument

journal.cli.host.parser.convey.argument.port.expected.one.argument = { $command }: error: argument { $flag }: expected one argument

journal.cli.host.parser.convey.argument.port.invalid.int.value = { $command }: error: argument { $flag }: invalid int value

journal.cli.host.parser.convey.argument.port.invalid.int.value.value = { $command }: error: argument { $flag }: invalid int value: '{ $value }'

journal.cli.host.parser.convey.argument.port.must.be.between.n1.and.n65535 = { $command }: error: argument { $flag }: must be between 1 and 65535

journal.cli.host.parser.convey.the.following.arguments.are.required.port = { $command }: error: the following arguments are required: { $flag }

journal.cli.host.parser.convey.unrecognized.arguments.arguments = { $command }: error: unrecognized arguments: { $arguments }

journal.cli.host.parser.cortex.unrecognized.arguments.arguments = { $command }: error: unrecognized arguments: { $arguments }

journal.cli.host.parser.direct.port.value.argument.direct.port.invalid.int.value.value = { $command }: error: argument { $flag }: invalid int value: '{ $value }'

journal.cli.host.parser.direct.port.value.argument.direct.port.must.be.between.n1.and.n65535 = { $command }: error: argument { $flag }: must be between 1 and 65535

journal.cli.host.parser.grab.argument.out.expected.one.argument = { $command }: error: argument { $flag }: expected one argument

journal.cli.host.parser.grab.path.tokens.must.be.valid.unicode = { $command }: error: path tokens must be valid Unicode

journal.cli.host.parser.grab.unrecognized.arguments.arguments = { $command }: error: unrecognized arguments: { $arguments }

journal.cli.host.parser.mcp.activity.expected.argument.after.name = { $command }: error: expected argument after { $name }

journal.cli.host.parser.mcp.activity.limit.must.be.between.n1.and.n1000 = { $command }: error: { $flag } must be between 1 and 1000

journal.cli.host.parser.mcp.activity.must.be.nonempty.name = { $command }: error: { $name } must be nonempty

journal.cli.host.parser.mcp.activity.unknown.option.accepted.token.oauth.created.tool.outcome.day.from.day.to.limit.json.other = { $command }: error: unknown option { $other }; accepted: { $flag }, { $flag_2 }, { $flag_3 }, { $flag_4 }, { $flag_5 }, { $flag_6 }, { $flag_7 }, { $flag_8 }, { $flag_9 }

journal.cli.host.parser.mcp.invalid.mcp.command.value = { $command }: error: invalid { $term } command: { $value }

journal.cli.host.parser.mcp.oauth.client.id.client.id.must.be.valid.utf.n8.and.nonempty = { $command }: error: client-id must be valid { $term } and nonempty

journal.cli.host.parser.mcp.oauth.client.id.expected.exactly.one.client.id.client.id.argument = { $command }: error: expected exactly one { $flag } { $argument } argument

journal.cli.host.parser.mcp.oauth.client.id.the.following.arguments.are.required.client.id.client.id = { $command }: error: the following arguments are required: { $flag } { $argument }

journal.cli.host.parser.mcp.oauth.invalid.mcp.oauth.command.value = { $command }: error: invalid { $term } oauth command: { $value }

journal.cli.host.parser.mcp.oauth.the.following.arguments.are.required.list.revoke = { $command }: error: the following arguments are required: { $literal }, { $literal_2 }

journal.cli.host.parser.mcp.pairing.expected.optional.door.local.lan.solstone.me.hostname.argument = { $command }: error: expected optional { $syntax } argument

journal.cli.host.parser.mcp.pairing.invalid.mcp.pairing.command.value = { $command }: error: invalid { $term } pairing command: { $value }

journal.cli.host.parser.mcp.pairing.the.following.arguments.are.required.generate.revoke = { $command }: error: the following arguments are required: { $literal }, { $literal_2 }

journal.cli.host.parser.mcp.pairing.unknown.door.choose.one.of.local.lan.solstone.me.hostname = { $command }: error: unknown door, choose one of { $literal }, { $literal_2 }, { $literal_3 }, { $literal_4 }

journal.cli.host.parser.mcp.permission.invalid.mcp.permission.command.value = { $command }: error: invalid { $term } permission command: { $value }

journal.cli.host.parser.mcp.permission.set.at.least.one.category.is.required = { $command }: error: at least one { $flag } is required

journal.cli.host.parser.mcp.permission.set.category.must.be.nonempty = { $command }: error: category must be nonempty

journal.cli.host.parser.mcp.permission.set.category.must.be.valid.utf.n8.and.nonempty = { $command }: error: category must be valid { $term } and nonempty

journal.cli.host.parser.mcp.permission.set.expected.argument.after.category = { $command }: error: expected argument after { $flag }

journal.cli.host.parser.mcp.permission.set.expected.argument.after.facet = { $command }: error: expected argument after { $flag }

journal.cli.host.parser.mcp.permission.set.expected.argument.after.places.precision = { $command }: error: expected argument after { $flag }

journal.cli.host.parser.mcp.permission.set.facet.name.must.be.nonempty = { $command }: error: facet name must be nonempty

journal.cli.host.parser.mcp.permission.set.facet.name.must.be.valid.utf.n8.and.nonempty = { $command }: error: facet name must be valid { $term } and nonempty

journal.cli.host.parser.mcp.permission.set.places.precision.must.be.nonempty = { $command }: error: places precision must be nonempty

journal.cli.host.parser.mcp.permission.set.places.precision.must.be.valid.utf.n8.and.nonempty = { $command }: error: places precision must be valid { $term } and nonempty

journal.cli.host.parser.mcp.permission.set.places.precision.repeated = { $command }: error: { $flag } may only be given once

journal.cli.host.parser.mcp.permission.the.following.arguments.are.required.show.set.clear = { $command }: error: the following arguments are required: { $literal }, { $literal_2 }, { $literal_3 }

journal.cli.host.parser.mcp.probe.arguments.is.required = { $command }: error: { $flag } is required

journal.cli.host.parser.mcp.probe.expected.argument.after.arguments = { $command }: error: expected argument after { $flag }

journal.cli.host.parser.mcp.probe.expected.argument.after.tool = { $command }: error: expected argument after { $flag }

journal.cli.host.parser.mcp.probe.tool.is.required = { $command }: error: { $flag } is required

journal.cli.host.parser.mcp.target.created.cannot.be.used.with.token = { $command }: error: { $flag } cannot be used with { $flag_2 }

journal.cli.host.parser.mcp.target.created.timestamp.must.be.nonempty = { $command }: error: created timestamp must be nonempty

journal.cli.host.parser.mcp.target.created.timestamp.must.be.valid.utf.n8.and.nonempty = { $command }: error: created timestamp must be valid { $term } and nonempty

journal.cli.host.parser.mcp.target.expected.argument.after.created = { $command }: error: expected argument after { $flag }

journal.cli.host.parser.mcp.target.expected.argument.after.oauth = { $command }: error: expected argument after { $flag }

journal.cli.host.parser.mcp.target.expected.argument.after.token = { $command }: error: expected argument after { $flag }

journal.cli.host.parser.mcp.target.oauth.client.id.must.be.nonempty = { $command }: error: oauth client-id must be nonempty

journal.cli.host.parser.mcp.target.oauth.client.id.must.be.valid.utf.n8.and.nonempty = { $command }: error: oauth client-id must be valid { $term } and nonempty

journal.cli.host.parser.mcp.target.one.of.token.or.oauth.is.required = { $command }: error: one of { $flag } or { $flag_2 } is required

journal.cli.host.parser.mcp.target.specify.either.token.or.oauth.not.both = { $command }: error: specify either { $flag } or { $flag_2 }, not both

journal.cli.host.parser.mcp.target.token.label.must.be.nonempty = { $command }: error: token label must be nonempty

journal.cli.host.parser.mcp.target.token.label.must.be.valid.utf.n8.and.nonempty = { $command }: error: token label must be valid { $term } and nonempty

journal.cli.host.parser.mcp.target.unexpected.argument.option = { $command }: error: unexpected argument: { $option }

journal.cli.host.parser.mcp.the.following.arguments.are.required.service.local.door.status.token.pairing.oauth.permission.activity.probe = { $command }: error: the following arguments are required: { $literal }, { $literal_2 }, { $literal_3 }, { $literal_4 }, { $literal_5 }, { $literal_6 }, { $literal_7 }, { $literal_8 }, { $literal_9 }

journal.cli.host.parser.mcp.token.invalid.mcp.token.command.value = { $command }: error: invalid { $term } token command: { $value }

journal.cli.host.parser.mcp.token.label.expected.exactly.one.label.label.argument = { $command }: error: expected exactly one { $flag } { $argument } argument

journal.cli.host.parser.mcp.token.label.label.must.be.valid.utf.n8.and.nonempty = { $command }: error: label must be valid { $term } and nonempty

journal.cli.host.parser.mcp.token.label.the.following.arguments.are.required.label.label = { $command }: error: the following arguments are required: { $flag } { $argument }

journal.cli.host.parser.mcp.token.the.following.arguments.are.required.create.list.revoke = { $command }: error: the following arguments are required: { $literal }, { $literal_2 }, { $literal_3 }

journal.cli.host.parser.schedule.unrecognized.arguments.arguments = { $command }: error: unrecognized arguments: { $arguments }

journal.cli.host.parser.spl.invalid.choice.choose.from.service.value = { $command }: error: invalid choice: { $value } (choose from { $literal })

journal.cli.host.parser.spl.the.following.arguments.are.required.service = { $command }: error: the following arguments are required: { $literal }

journal.cli.host.parser.spl.unrecognized.arguments.arguments = { $command }: error: unrecognized arguments: { $arguments }

journal.cli.host.parser.supervisor.argument.direct.port.cannot.be.repeated = { $command }: error: argument { $flag }: cannot be repeated

journal.cli.host.parser.supervisor.argument.direct.port.expected.one.argument = { $command }: error: argument { $flag }: expected one argument

journal.cli.host.parser.supervisor.argument.direct.port.invalid.int.value = { $command }: error: argument { $flag }: invalid int value

journal.cli.host.parser.validate.day.day.must.be.eight.digits.yyyymmdd.got.value = { $command }: error: day must be eight digits ({ $argument }), got { $value }

journal.cli.host.process.admission = journal process admission failed: { $error }

journal.cli.host.read.error.error.solstone.observe.utils.error.reading = { $literal_error_solstone_observe_utils }:Error reading { $path }: { $error }

journal.cli.host.render.oauth.store.error.no.active.pairing.code = { $command_solstone_journal_mcp } { $command } { $operation }: no active pairing code

journal.cli.host.render.oauth.store.error.no.such.oauth.client = { $command_solstone_journal_mcp } { $command } { $operation }: no such { $literal_oauth } client

journal.cli.host.render.supervisor.host.outcome.supervisor.failed.to.boot = supervisor failed to boot: { $reason }

journal.cli.host.render.supervisor.host.outcome.supervisor.lifecycle.shutdown.failed.cause.readiness.self.heartbeat.identity = supervisor lifecycle shutdown failed: cause={ $cause }, readiness={ $readiness }, self_heartbeat={ $self_heartbeat }, identity={ $identity }

journal.cli.host.render.token.store.error.token = { $command_solstone_journal_mcp_token } { $operation }: { $error }

journal.cli.host.render.token.store.error.token.a.bearer.token.already.exists.for.label = { $command_solstone_journal_mcp_token } { $operation }: a bearer token already exists for label { $label }

journal.cli.host.render.token.store.error.token.invalid.label = { $command_solstone_journal_mcp_token } { $operation }: invalid label: { $error }

journal.cli.host.render.token.store.error.token.no.bearer.token.exists.for.label = { $command_solstone_journal_mcp_token } { $operation }: no bearer token exists for label { $label }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.failed.to.query.oauth.store = { $command_solstone_journal_mcp }: failed to query oauth store: { $e }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.failed.to.query.token.store = { $command_solstone_journal_mcp }: failed to query token store: { $e }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.multiple.oauth.grants.matched.client.id.specify = { $command_solstone_journal_mcp }: multiple { $literal_oauth } grants matched { $literal_client_id } { $client_id }; specify { $literal_created } { $timestamp_arg }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.no.bearer.token.exists.for.label = { $command_solstone_journal_mcp }: no bearer token exists for label { $label }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.no.oauth.grant.found.matching.client.id = { $command_solstone_journal_mcp }: no { $literal_oauth } grant found matching { $literal_client_id } { $client_id }

journal.cli.host.schedule.help = usage: { $syntax }{ $line_break }{ $line_break }Show scheduled tasks{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }Enable verbose output{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Enable debug logging{ $line_break }

journal.cli.host.schedule.usage = usage: { $syntax }{ $line_break }

journal.cli.host.sense.help = usage: { $syntax }{ $line_break }{ $spaces_20 }{ $syntax_4 }{ $line_break }{ $spaces_20 }{ $syntax_5 }{ $line_break }{ $line_break }Unified observe file processor{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_13 }Process files from specific day ({ $argument } format) instead of watching{ $line_break }{ $spaces_2 }{ $option_12 }{ $spaces_2 }Max concurrent screen-describe jobs when using { $flag } (default: 1).{ $line_break }{ $spaces_2 }{ $flag_14 } { $choices }{ $line_break }{ $spaces_24 }Delete existing screen or audio outputs and reprocess (requires { $flag }){ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_5 }Filter to specific segment ({ $argument_19 } format, requires { $flag }){ $line_break }{ $spaces_2 }{ $option_20 }{ $spaces_7 }Filter to specific stream (requires { $flag }){ $line_break }{ $spaces_2 }{ $option_22 }{ $spaces_13 }Show what would be processed (or deleted with { $flag_14 }) without making changes{ $line_break }{ $spaces_2 }{ $option_23 }{ $spaces_9 }Enable verbose output{ $line_break }{ $spaces_2 }{ $option_25 }{ $spaces_11 }Enable debug logging{ $line_break }

journal.cli.host.sense.sense.batch.failed = sense batch failed: { $error }

journal.cli.host.sense.sense.parent.loss.admission.failed = sense parent-loss admission failed: { $error }

journal.cli.host.sense.service.sense.service.failed = sense service failed: { $class }

journal.cli.host.sense.usage = usage: { $syntax }{ $line_break }{ $spaces_20 }{ $syntax_4 }{ $line_break }{ $spaces_20 }{ $syntax_5 }{ $line_break }

journal.cli.host.service.argument.port = error: unexpected argument '{ $text }'; pass the port as { $flag } { $port }

journal.cli.host.service.argument.range = error: unexpected argument '{ $text }'; pass a port between 1 and 65535 as { $flag } { $argument }

journal.cli.host.service.argument.unexpected = error: unexpected argument '{ $text }'; pass the port as { $flag } { $argument }

journal.cli.host.service.conflicting.absent.systemd.state = conflicting absent { $systemd } state

journal.cli.host.service.could.not.clear.the.service.s.ready.state = could not clear the service's ready state: { $error }

journal.cli.host.service.could.not.find.the.solstone.installation.from = could not find the { $solstone } installation from { $path }

journal.cli.host.service.could.not.resolve.journal = could not resolve journal

journal.cli.host.service.could.not.resolve.the.solstone.executable = could not resolve the { $solstone } executable: { $error }

journal.cli.host.service.could.not.verify.the.saved.installation = could not verify the saved installation: { $error }

journal.cli.host.service.could.not.verify.the.solstone.installation = could not verify the { $solstone } installation: { $error }

journal.cli.host.service.could.not.verify.the.solstone.setup.storage = could not verify the { $solstone } setup storage: { $error }

journal.cli.host.service.couldn.t.confirm.that.the.service.uses.this.journal = couldn't confirm that the service uses this journal. run '{ $setup }' to update its saved location.

journal.cli.host.service.create.service.directory = create service directory: { $source }

journal.cli.host.service.create.service.health.directory = create service health directory: { $source }

journal.cli.host.service.duplicate.systemd.property = duplicate { $systemd } property: { $key }

journal.cli.host.service.entry.is.not.a.regular.file = entry is not a regular file

journal.cli.host.service.error.details = error: { $detail }

journal.cli.host.service.executable.parent = service executable has no parent

journal.cli.host.service.file.absent = { $operation ->
    [change-journal-folder] service change journal folder refused { $path }: absent
    [install] service install refused { $path }: absent
    [operate] service operate refused { $path }: absent
    [restart] service restart refused { $path }: absent
    [route-repair] service route repair refused { $path }: absent
    [status] service status refused { $path }: absent
    [stop] service stop refused { $path }: absent
   *[uninstall] service uninstall refused { $path }: absent
    }

journal.cli.host.service.file.foreign = { $operation ->
    [change-journal-folder] service change journal folder refused { $path }: not recognized as a service file created by journal
    [install] service install refused { $path }: not recognized as a service file created by journal
    [operate] service operate refused { $path }: not recognized as a service file created by journal
    [restart] service restart refused { $path }: not recognized as a service file created by journal
    [route-repair] service route repair refused { $path }: not recognized as a service file created by journal
    [status] service status refused { $path }: not recognized as a service file created by journal
    [stop] service stop refused { $path }: not recognized as a service file created by journal
   *[uninstall] service uninstall refused { $path }: not recognized as a service file created by journal
    }

journal.cli.host.service.file.managed = { $operation ->
    [change-journal-folder] service change journal folder refused { $path }: managed
    [install] service install refused { $path }: managed
    [operate] service operate refused { $path }: managed
    [restart] service restart refused { $path }: managed
    [route-repair] service route repair refused { $path }: managed
    [status] service status refused { $path }: managed
    [stop] service stop refused { $path }: managed
   *[uninstall] service uninstall refused { $path }: managed
    }

journal.cli.host.service.file.unknown = { $operation ->
    [change-journal-folder] service change journal folder refused { $path }: could not verify the service file: { $cause }
    [install] service install refused { $path }: could not verify the service file: { $cause }
    [operate] service operate refused { $path }: could not verify the service file: { $cause }
    [restart] service restart refused { $path }: could not verify the service file: { $cause }
    [route-repair] service route repair refused { $path }: could not verify the service file: { $cause }
    [status] service status refused { $path }: could not verify the service file: { $cause }
    [stop] service stop refused { $path }: could not verify the service file: { $cause }
   *[uninstall] service uninstall refused { $path }: could not verify the service file: { $cause }
    }

journal.cli.host.service.incomplete.systemd.property.set = incomplete { $systemd } property set

journal.cli.host.service.inspect.launchd.directory = inspect { $launchd } directory: { $error }

journal.cli.host.service.inspect.service.directory = inspect service directory: { $error }

journal.cli.host.service.inspect.service.health.directory = inspect service health directory: { $error }

journal.cli.host.service.inspect.service.journal.root = inspect service journal root: { $error }

journal.cli.host.service.inspect.stale.launchd.unit = inspect stale { $launchd } unit: { $error }

journal.cli.host.service.installation.changed.while.waiting.for.the.new.journal.folder = installation changed while waiting for the new journal folder

journal.cli.host.service.launchd.accepted.the.unload.request.but.the.service.is = { $launchd } accepted the unload request, but the service is still present

journal.cli.host.service.launchd.directory.is.not.a.real.directory = { $launchd } directory is not a real directory

journal.cli.host.service.launchd.observation.was.not.utf = { $launchd } observation was not UTF-8

journal.cli.host.service.launchd.omitted.or.malformed.arguments = { $launchd } omitted or malformed arguments

journal.cli.host.service.launchd.omitted.the.runtime.state = { $launchd } omitted the runtime state

journal.cli.host.service.malformed.systemd.property = malformed { $systemd } property

journal.cli.host.service.manager.disable.service = disable service failed: { $result }

journal.cli.host.service.manager.disable.stale.service = disable stale service failed: { $result }

journal.cli.host.service.manager.enable.service = enable service failed: { $result }

journal.cli.host.service.manager.load.installed.service = load installed service failed: { $result }

journal.cli.host.service.manager.load.service = load service failed: { $result }

journal.cli.host.service.manager.load.service.after.changing.journal.folder = load service after changing journal folder failed: { $result }

journal.cli.host.service.manager.reload.service.after.changing.journal.folder = reload service after changing journal folder failed: { $result }

journal.cli.host.service.manager.reload.systemd = reload systemd failed: { $result }

journal.cli.host.service.manager.reload.systemd.for.route.repair = reload systemd for route repair failed: { $result }

journal.cli.host.service.manager.request.launchd.unload = request launchd unload failed: { $result }

journal.cli.host.service.manager.restart.service = restart service failed: { $result }

journal.cli.host.service.manager.restart.service.for.route.repair = restart service for route repair failed: { $result }

journal.cli.host.service.manager.start.after.changing.journal.folder = start after changing journal folder failed: { $result }

journal.cli.host.service.manager.start.service = start service failed: { $result }

journal.cli.host.service.manager.stop.service = stop service failed: { $result }

journal.cli.host.service.manager.stop.service.before.changing.journal.folder = stop service before changing journal folder failed: { $result }

journal.cli.host.service.manager.stop.service.before.restart = stop service before restart failed: { $result }

journal.cli.host.service.manager.stop.stale.service = stop stale service failed: { $result }

journal.cli.host.service.manager.unload.stale.launchd.service = unload stale launchd service failed: { $result }

journal.cli.host.service.manager.unload.stale.service = unload stale service failed: { $result }

journal.cli.host.service.metadata = metadata: { $error }

journal.cli.host.service.open.your.journal = open your journal: { $url }

journal.cli.host.service.open.your.journal.solstone.journal.open = open your journal: { $open }

journal.cli.host.service.parse.stale.launchd.unit = parse stale { $launchd } unit: { $error }

journal.cli.host.service.port.invalid = error: invalid port '{ $value }'

journal.cli.host.service.preserved.service.unit = preserved service unit { $path }

journal.cli.host.service.read = read: { $error }

journal.cli.host.service.ready.timeout = service did not become ready within 120 seconds

journal.cli.host.service.recheck.stale.launchd.unit = recheck stale { $launchd } unit: { $error }

journal.cli.host.service.registration.a.different.unit.answers.to.this.name = { $operation ->
    [change-journal-folder] service change journal folder refused: a different unit answers to this name. journal will not overwrite it. If that is deliberate, finish the change journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [clean-stale-launchd-service] service clean stale launchd service refused: a different unit answers to this name. journal will not overwrite it. If that is deliberate, finish the clean stale launchd service yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [install] service install refused: a different unit answers to this name. journal will not overwrite it. If that is deliberate, finish the install yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [operate] service operate refused: a different unit answers to this name. journal will not overwrite it. If that is deliberate, finish the operate yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [start-after-changing-journal-folder] service start after changing journal folder refused: a different unit answers to this name. journal will not overwrite it. If that is deliberate, finish the start after changing journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [status] service status refused: a different unit answers to this name. journal will not overwrite it. If that is deliberate, finish the status yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [stop] service stop refused: a different unit answers to this name. journal will not overwrite it. If that is deliberate, finish the stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [uninstall] service uninstall refused: a different unit answers to this name. journal will not overwrite it. If that is deliberate, finish the uninstall yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [up] service up refused: a different unit answers to this name. journal will not overwrite it. If that is deliberate, finish the up yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [verify-service-stop] service verify service stop refused: a different unit answers to this name. journal will not overwrite it. If that is deliberate, finish the verify service stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
   *[verify-unload] service verify unload refused: a different unit answers to this name. journal will not overwrite it. If that is deliberate, finish the verify unload yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.launch.agent.is.not.the.one.journal.manages = { $operation ->
    [change-journal-folder] service change journal folder refused: the launch agent is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the change journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [clean-stale-launchd-service] service clean stale launchd service refused: the launch agent is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the clean stale launchd service yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [install] service install refused: the launch agent is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the install yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [operate] service operate refused: the launch agent is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the operate yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [start-after-changing-journal-folder] service start after changing journal folder refused: the launch agent is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the start after changing journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [status] service status refused: the launch agent is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the status yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [stop] service stop refused: the launch agent is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [uninstall] service uninstall refused: the launch agent is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the uninstall yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [up] service up refused: the launch agent is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the up yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [verify-service-stop] service verify service stop refused: the launch agent is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the verify service stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
   *[verify-unload] service verify unload refused: the launch agent is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the verify unload yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.launch.agent.s.program.is.not.a.journal = { $operation ->
    [change-journal-folder] service change journal folder refused: the launch agent's program is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the change journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [clean-stale-launchd-service] service clean stale launchd service refused: the launch agent's program is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the clean stale launchd service yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [install] service install refused: the launch agent's program is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the install yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [operate] service operate refused: the launch agent's program is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the operate yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [start-after-changing-journal-folder] service start after changing journal folder refused: the launch agent's program is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the start after changing journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [status] service status refused: the launch agent's program is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the status yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [stop] service stop refused: the launch agent's program is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [uninstall] service uninstall refused: the launch agent's program is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the uninstall yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [up] service up refused: the launch agent's program is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the up yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [verify-service-stop] service verify service stop refused: the launch agent's program is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the verify service stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
   *[verify-unload] service verify unload refused: the launch agent's program is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the verify unload yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.unit.carries.a.drop.in.journal.did.not = { $operation ->
    [change-journal-folder] service change journal folder refused: the unit carries a drop-in journal did not write. journal will not overwrite it. If that is deliberate, finish the change journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [clean-stale-launchd-service] service clean stale launchd service refused: the unit carries a drop-in journal did not write. journal will not overwrite it. If that is deliberate, finish the clean stale launchd service yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [install] service install refused: the unit carries a drop-in journal did not write. journal will not overwrite it. If that is deliberate, finish the install yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [operate] service operate refused: the unit carries a drop-in journal did not write. journal will not overwrite it. If that is deliberate, finish the operate yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [start-after-changing-journal-folder] service start after changing journal folder refused: the unit carries a drop-in journal did not write. journal will not overwrite it. If that is deliberate, finish the start after changing journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [status] service status refused: the unit carries a drop-in journal did not write. journal will not overwrite it. If that is deliberate, finish the status yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [stop] service stop refused: the unit carries a drop-in journal did not write. journal will not overwrite it. If that is deliberate, finish the stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [uninstall] service uninstall refused: the unit carries a drop-in journal did not write. journal will not overwrite it. If that is deliberate, finish the uninstall yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [up] service up refused: the unit carries a drop-in journal did not write. journal will not overwrite it. If that is deliberate, finish the up yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [verify-service-stop] service verify service stop refused: the unit carries a drop-in journal did not write. journal will not overwrite it. If that is deliberate, finish the verify service stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
   *[verify-unload] service verify unload refused: the unit carries a drop-in journal did not write. journal will not overwrite it. If that is deliberate, finish the verify unload yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.unit.file.is.not.the.one.journal.manages = { $operation ->
    [change-journal-folder] service change journal folder refused: the unit file is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the change journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [clean-stale-launchd-service] service clean stale launchd service refused: the unit file is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the clean stale launchd service yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [install] service install refused: the unit file is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the install yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [operate] service operate refused: the unit file is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the operate yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [start-after-changing-journal-folder] service start after changing journal folder refused: the unit file is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the start after changing journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [status] service status refused: the unit file is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the status yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [stop] service stop refused: the unit file is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [uninstall] service uninstall refused: the unit file is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the uninstall yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [up] service up refused: the unit file is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the up yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [verify-service-stop] service verify service stop refused: the unit file is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the verify service stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
   *[verify-unload] service verify unload refused: the unit file is not the one journal manages. journal will not overwrite it. If that is deliberate, finish the verify unload yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.unit.is.in.an.unexpected.state = { $operation ->
    [change-journal-folder] service change journal folder refused: the unit is in an unexpected state. journal will not overwrite it. If that is deliberate, finish the change journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [clean-stale-launchd-service] service clean stale launchd service refused: the unit is in an unexpected state. journal will not overwrite it. If that is deliberate, finish the clean stale launchd service yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [install] service install refused: the unit is in an unexpected state. journal will not overwrite it. If that is deliberate, finish the install yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [operate] service operate refused: the unit is in an unexpected state. journal will not overwrite it. If that is deliberate, finish the operate yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [start-after-changing-journal-folder] service start after changing journal folder refused: the unit is in an unexpected state. journal will not overwrite it. If that is deliberate, finish the start after changing journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [status] service status refused: the unit is in an unexpected state. journal will not overwrite it. If that is deliberate, finish the status yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [stop] service stop refused: the unit is in an unexpected state. journal will not overwrite it. If that is deliberate, finish the stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [uninstall] service uninstall refused: the unit is in an unexpected state. journal will not overwrite it. If that is deliberate, finish the uninstall yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [up] service up refused: the unit is in an unexpected state. journal will not overwrite it. If that is deliberate, finish the up yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [verify-service-stop] service verify service stop refused: the unit is in an unexpected state. journal will not overwrite it. If that is deliberate, finish the verify service stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
   *[verify-unload] service verify unload refused: the unit is in an unexpected state. journal will not overwrite it. If that is deliberate, finish the verify unload yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.unit.s.execstart.is.not.a.journal.launcher = { $operation ->
    [change-journal-folder] service change journal folder refused: the unit's ExecStart is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the change journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [clean-stale-launchd-service] service clean stale launchd service refused: the unit's ExecStart is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the clean stale launchd service yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [install] service install refused: the unit's ExecStart is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the install yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [operate] service operate refused: the unit's ExecStart is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the operate yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [start-after-changing-journal-folder] service start after changing journal folder refused: the unit's ExecStart is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the start after changing journal folder yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [status] service status refused: the unit's ExecStart is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the status yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [stop] service stop refused: the unit's ExecStart is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [uninstall] service uninstall refused: the unit's ExecStart is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the uninstall yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [up] service up refused: the unit's ExecStart is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the up yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    [verify-service-stop] service verify service stop refused: the unit's ExecStart is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the verify service stop yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
   *[verify-unload] service verify unload refused: the unit's ExecStart is not a journal launcher. journal will not overwrite it. If that is deliberate, finish the verify unload yourself -- on Linux: `{ $restart_command }` -- then confirm the running build with `{ $inspect_command }`
    }

journal.cli.host.service.registration.unknown = { $operation ->
    [change-journal-folder] service change journal folder could not check the service registration: { $cause }
    [clean-stale-launchd-service] service clean stale launchd service could not check the service registration: { $cause }
    [install] service install could not check the service registration: { $cause }
    [operate] service operate could not check the service registration: { $cause }
    [start-after-changing-journal-folder] service start after changing journal folder could not check the service registration: { $cause }
    [status] service status could not check the service registration: { $cause }
    [stop] service stop could not check the service registration: { $cause }
    [uninstall] service uninstall could not check the service registration: { $cause }
    [up] service up could not check the service registration: { $cause }
    [verify-service-stop] service verify service stop could not check the service registration: { $cause }
   *[verify-unload] service verify unload could not check the service registration: { $cause }
    }

journal.cli.host.service.remove.service.unit = remove service unit: { $error }

journal.cli.host.service.remove.stale.launchd.unit = remove stale { $launchd } unit: { $error }

journal.cli.host.service.removed = removed { $path }

journal.cli.host.service.removed.stale.launchd.plist = removed stale { $launchd } plist { $path }

journal.cli.host.service.removed.stale.service.registration = removed stale service registration

journal.cli.host.service.route.repair.service.launcher.is.not.an.executable.regular = route repair service launcher is not an executable regular file

journal.cli.host.service.route.repair.service.launcher.is.unavailable = route repair service launcher is unavailable: { $error }

journal.cli.host.service.route.repair.service.unit.has.an.invalid.command.tail = route repair service unit has an invalid command tail

journal.cli.host.service.route.repair.service.unit.has.an.invalid.execstart = route repair service unit has an invalid { $exec_start }

journal.cli.host.service.route.repair.service.unit.has.no.execstart = route repair service unit has no { $exec_start }

journal.cli.host.service.route.repair.service.unit.is.not.utf = route repair service unit is not UTF-8

journal.cli.host.service.run.solstone.journal.setup.or.solstone.journal.service.install = run '{ $setup }' or '{ $install }' to install it.

journal.cli.host.service.scan.launchd.directory = scan { $launchd } directory: { $error }

journal.cli.host.service.scan.launchd.entry = scan { $launchd } entry: { $error }

journal.cli.host.service.service.became.active.during.the.journal.folder.change = service became active during the journal-folder change

journal.cli.host.service.service.command.output.exceeded.kib = service command output exceeded 256 KiB

journal.cli.host.service.service.command.output.failed = service command output failed: { $error }

journal.cli.host.service.service.command.spawn.failed = service command spawn failed: { $error }

journal.cli.host.service.service.command.timed.out = service command timed out

journal.cli.host.service.service.command.wait.failed = service command wait failed: { $error }

journal.cli.host.service.service.directory.contains.unsafe.component = service directory contains unsafe component

journal.cli.host.service.service.directory.escaped.home = service directory escaped home

journal.cli.host.service.service.enabled = service enabled

journal.cli.host.service.service.health.path.is.not.a.real.directory = service health path is not a real directory

journal.cli.host.service.service.home = service home: { $error }

journal.cli.host.service.service.install.could.not.resolve.journal = service install: could not resolve journal

journal.cli.host.service.service.install.current.executable.has.no.directory = service install: current executable has no directory

journal.cli.host.service.service.install.invalid.installation.identity.arguments = service install: invalid installation identity arguments: { $error }

journal.cli.host.service.service.install.requires.installation.identity.arguments = service install requires installation identity arguments

journal.cli.host.service.service.installed = service: installed

journal.cli.host.service.service.journal.root.is.not.a.real.directory = service journal root is not a real directory

journal.cli.host.service.service.launcher.is.not.an.executable.regular.file = service launcher is not an executable regular file

journal.cli.host.service.service.launcher.is.unavailable = service launcher is unavailable: { $error }

journal.cli.host.service.service.loaded.into.launchd = service loaded into { $launchd }

journal.cli.host.service.service.lock = service lock: { $error }

journal.cli.host.service.service.management.is.supported.only.on.linux.and.mac = service management is supported only on linux and mac

journal.cli.host.service.service.not.installed = service: not installed

journal.cli.host.service.service.not.installed.run.solstone.journal.service.install.first = service not installed. run '{ $install }' first.

journal.cli.host.service.service.path.is.not.utf = service path is not UTF-8

journal.cli.host.service.service.registration.has.no.matching.managed.file = service registration has no matching managed file

journal.cli.host.service.service.registration.has.no.valid.port = service registration has no valid port

journal.cli.host.service.service.restarted = service restarted

journal.cli.host.service.service.runtime.state.was.unexpected = { $operation ->
    [change-journal-folder] service change journal folder runtime state was unexpected
    [clean-stale-launchd-service] service clean stale launchd service runtime state was unexpected
    [install] service install runtime state was unexpected
    [operate] service operate runtime state was unexpected
    [start-after-changing-journal-folder] service start after changing journal folder runtime state was unexpected
    [status] service status runtime state was unexpected
    [stop] service stop runtime state was unexpected
    [uninstall] service uninstall runtime state was unexpected
    [up] service up runtime state was unexpected
    [verify-service-stop] service verify service stop runtime state was unexpected
   *[verify-unload] service verify unload runtime state was unexpected
    }

journal.cli.host.service.service.started = service started

journal.cli.host.service.service.stderr.reader.panicked = service stderr reader panicked

journal.cli.host.service.service.stderr.unavailable = service stderr unavailable

journal.cli.host.service.service.stdout.reader.panicked = service stdout reader panicked

journal.cli.host.service.service.stdout.unavailable = service stdout unavailable

journal.cli.host.service.service.stopped = service stopped

journal.cli.host.service.service.stopped.but.its.ready.state.could.not.be = service stopped, but its ready state could not be cleared: { $error }

journal.cli.host.service.service.unit.changed.before.publication.nothing.was.written = service unit changed before publication; nothing was written

journal.cli.host.service.service.unit.changed.during.operation.preserved.replacement = service unit changed during operation; preserved replacement

journal.cli.host.service.service.unit.has.no.parent = service unit has no parent

journal.cli.host.service.service.unit.has.no.trusted.home.ancestor = service unit has no trusted home ancestor

journal.cli.host.service.service.unit.publication.failed = service unit publication failed: { $error }

journal.cli.host.service.service.unit.published.with.uncertain.durability = service unit published with uncertain durability: { $other }

journal.cli.host.service.service.was.not.installed = service was not installed

journal.cli.host.service.sign.in.invalid = error: { $flag } expects { $literal } or { $literal_3 }

journal.cli.host.service.stale.launchd.plist.changed.during.cleanup.preserved = stale { $launchd } plist changed during cleanup; preserved { $path }

journal.cli.host.service.stale.launchd.unit.has.a.foreign.label = stale { $launchd } unit has a foreign label

journal.cli.host.service.stale.launchd.unit.has.conflicting.program.fields = stale { $launchd } unit has conflicting program fields

journal.cli.host.service.stale.launchd.unit.has.no.label = stale { $launchd } unit has no label

journal.cli.host.service.stale.launchd.unit.has.no.launcher = stale { $launchd } unit has no launcher

journal.cli.host.service.stale.launchd.unit.is.foreign.or.unrecognized = stale { $launchd } unit is foreign or unrecognized

journal.cli.host.service.stale.launchd.unit.is.not.a.dictionary = stale { $launchd } unit is not a dictionary

journal.cli.host.service.state.running = state: running ({ $manager })

journal.cli.host.service.state.stopped = state: stopped

journal.cli.host.service.subcommand.unknown = unknown subcommand: { $value }; available: { $literal }

journal.cli.host.service.systemd.observation.was.not.utf = { $systemd } observation was not UTF-8

journal.cli.host.service.the.previous.service.did.not.stop.before.replacement = the previous service did not stop before replacement

journal.cli.host.service.the.solstone.executable.has.no.containing.directory = the { $solstone } executable has no containing directory

journal.cli.host.service.unit.exceeds.mib = unit exceeds 1 MiB

journal.cli.host.service.unit.path.has.no.trusted.home.ancestor = unit path has no trusted home ancestor

journal.cli.host.service.unrecognized.systemd.active.substate.pair = unrecognized { $systemd } active/substate pair

journal.cli.host.service.unsafe.service.directory = unsafe service directory: { $path }

journal.cli.host.service.usage = Usage: { $syntax }{ $line_break }{ $spaces_7 }{ $syntax_4 }{ $spaces_2 }(default: 5015; { $term } only){ $line_break }{ $spaces_7 }{ $syntax_7 }{ $spaces_2 }(restart; { $flag } noops if not installed){ $line_break }{ $spaces_7 }{ $syntax_9 }{ $spaces_29 }(start + status; service must be installed){ $line_break }{ $spaces_7 }{ $syntax_11 }{ $spaces_27 }(stop){ $line_break }

journal.cli.host.service.windows.only = this service command is only available on Windows

journal.cli.host.service.wrote = wrote { $path }

journal.cli.host.settings.bind = convey{ $line_break }{ $space_2 }bind:{ $space_14 }{ $address }

journal.cli.host.settings.convey.error.argument.convey.command.invalid.choice.choose.from.status = { $command }: error: argument { $literal }: invalid choice: '{ $value }' (choose from { $literal_3 })

journal.cli.host.settings.convey.help = usage: { $syntax }{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_4 }Show convey bind and dashboard { $term } status{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }

journal.cli.host.settings.convey.usage = usage: { $syntax }{ $line_break }

journal.cli.host.settings.error.argument.section.invalid.choice.choose.from.convey = { $command }: error: argument { $literal }: invalid choice: '{ $value }' (choose from { $literal_3 })

journal.cli.host.settings.error.unrecognized.arguments = { $command }: error: unrecognized arguments: { $value }

journal.cli.host.settings.help = usage: { $syntax }{ $line_break }{ $line_break }Manage local journal settings{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_7 }Manage convey settings{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_2 }Enable verbose output{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_4 }Enable debug logging{ $line_break }

journal.cli.host.settings.open = open it with:{ $space_6 }{ $command }

journal.cli.host.settings.port.read = Error: { $path }: { $detail }

journal.cli.host.settings.status.help = usage: { $syntax }{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_5 }{ $spaces_6 }Print machine-readable status.{ $line_break }

journal.cli.host.settings.url = dashboard url:{ $space_5 }{ $url }

journal.cli.host.settings.usage = usage: { $syntax }{ $line_break }

journal.cli.host.setup.error = { $literal }: error: { $detail }

journal.cli.host.setup.usage = usage: { $syntax }{ $line_break }{ $spaces_21 }{ $syntax_4 }{ $line_break }{ $spaces_21 }{ $syntax_5 }{ $line_break }{ $spaces_21 }{ $syntax_6 }{ $line_break }{ $spaces_21 }{ $syntax_7 }{ $line_break }{ $spaces_21 }{ $syntax_8 }{ $line_break }{ $spaces_21 }{ $syntax_9 } ({ $term } only){ $line_break }

journal.cli.host.spl.help = usage: { $syntax }{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }Enable verbose output{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Enable debug logging{ $line_break }

journal.cli.host.spl.service.spl.service.failed = spl service failed: { $class }

journal.cli.host.spl.usage = usage: { $syntax }{ $line_break }

journal.cli.host.start.help = usage: { $syntax }{ $line_break }{ $spaces_21 }{ $syntax_4 }{ $line_break }{ $spaces_21 }{ $syntax_5 }{ $line_break }{ $spaces_21 }{ $syntax_6 }{ $line_break }{ $spaces_21 }{ $syntax_7 }{ $line_break }{ $line_break }Monitor journal system health{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_15 }{ $term } port (0 = auto-select available port){ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_9 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_9 }Disable daily processing run at midnight{ $line_break }{ $spaces_2 }{ $option_15 }{ $spaces_8 }Do not start the { $term_17 } server (run it manually for{ $line_break }{ $spaces_21 }debugging){ $line_break }{ $spaces_2 }{ $option_18 }{ $spaces_11 }Do not start the private network relay{ $line_break }{ $spaces_2 }{ $option_20 }{ $spaces_8 }Do not start the { $term } web application{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_6 }Do not initialize or run the schedule engine{ $line_break }{ $spaces_2 }{ $option_23 }{ $spaces_4 }Require the direct parent process to remain live{ $line_break }{ $spaces_2 }{ $option_25 }{ $spaces_2 }Use this path as the journal root instead of normal{ $line_break }{ $spaces_21 }journal resolution.{ $line_break }{ $spaces_2 }{ $flag } { $argument }{ $line_break }{ $spaces_21 }Paired-device door port (default: 7657){ $line_break }{ $spaces_2 }{ $option_28 }{ $spaces_6 }Enable verbose output{ $line_break }{ $spaces_2 }{ $option_29 }{ $spaces_8 }Enable debug logging{ $line_break }

journal.cli.host.start.usage = usage: { $syntax }{ $line_break }{ $spaces_21 }{ $syntax_4 }{ $line_break }{ $spaces_21 }{ $syntax_5 }{ $line_break }{ $spaces_21 }{ $syntax_6 }{ $line_break }{ $spaces_21 }{ $syntax_7 }{ $line_break }

journal.cli.host.supervisor.admission.wait.active = another { $brand } start is waiting after finding recent journal activity.{ $break_line }this start did not continue while that wait was active.{ $break_line }wait a moment, then try again.

journal.cli.host.supervisor.admission.wait.terminal = a recent heartbeat from another run is present.{ $break_line }the solstone app did not start while that heartbeat was still present.{ $break_line }wait a moment, then try again.

journal.cli.host.supervisor.admission.wait.unverifiable = startup status couldn't be verified.{ $break_line }wait a moment, then try again.

journal.cli.host.supervisor.already.running = your journal is already running.{ $break_line }{ $break_line }this start stepped aside so the running one is not disturbed. nothing needs doing.{ $break_line }

journal.cli.host.supervisor.binding.home = home: { $error }

journal.cli.host.supervisor.binding.installation.root = installation root: { $error }

journal.cli.host.supervisor.binding.journal.token = journal token: { $error }

journal.cli.host.supervisor.binding.owner.storage = owner storage: { $error }

journal.cli.host.supervisor.binding.root.token = root token: { $error }

journal.cli.host.supervisor.binding.saved.binding = saved binding: { $error }

journal.cli.host.supervisor.binding.unreadable = the saved installation binding could not be loaded

journal.cli.host.supervisor.help = usage: { $syntax }{ $line_break }{ $spaces_26 }{ $syntax_4 }{ $line_break }{ $spaces_26 }{ $syntax_5 }{ $line_break }{ $spaces_26 }{ $syntax_6 }{ $line_break }{ $spaces_26 }{ $syntax_7 }{ $line_break }{ $line_break }Monitor journal system health{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_15 }{ $term } port (0 = auto-select available port){ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_9 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_9 }Disable daily processing run at midnight{ $line_break }{ $spaces_2 }{ $option_15 }{ $spaces_8 }Do not start the { $term_17 } server (run it manually for{ $line_break }{ $spaces_21 }debugging){ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_11 }Do not start the private network relay{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_8 }Do not start the { $term } web application{ $line_break }{ $spaces_2 }{ $option_22 }{ $spaces_6 }Do not initialize or run the schedule engine{ $line_break }{ $spaces_2 }{ $option_24 }{ $spaces_2 }Use this path as the journal root instead of normal{ $line_break }{ $spaces_21 }journal resolution.{ $line_break }{ $spaces_2 }{ $flag } { $argument }{ $line_break }{ $spaces_21 }Paired-device door port (default: 7657){ $line_break }{ $spaces_2 }{ $option_27 }{ $spaces_6 }Enable verbose output{ $line_break }{ $spaces_2 }{ $option_28 }{ $spaces_8 }Enable debug logging{ $line_break }

journal.cli.host.supervisor.journal.program.folder = Reinstalling or uninstalling removes this program folder. Choose a journal location outside it. If your journal is already there, quit the journal app. If setup completed, run `{ $stop }`, then `{ $uninstall }` and wait for it to succeed. Move the whole journal folder outside the program folder before reinstalling or uninstalling, then select its new location with `{ $setup }`, replacing { $path } with its new location.

journal.cli.host.supervisor.journal.token.mismatch = this installation couldn't be verified.{ $line_break }run `{ $setup }` to check it. if setup finishes successfully, try again.{ $line_break }details: the saved installation binding is for a different journal

journal.cli.host.supervisor.lifecycle.redirect = { $supervisor } is the server-launch command (takes a port). For lifecycle, use: { $service_verb }. Did you mean: { $requested } ?

journal.cli.host.supervisor.recovery.closing = this start could not continue.{ $line_break }{ $line_break }the last run is still shutting down. wait a moment, then try again.{ $line_break }{ $line_break }your memories are untouched.{ $line_break }

journal.cli.host.supervisor.recovery.coordinator.retirement.unverified = details: a helper process could not be confirmed stopped

journal.cli.host.supervisor.recovery.coordinator.stopped.before.ready = details: a helper process stopped before it was ready

journal.cli.host.supervisor.recovery.identity.establishment = details: a helper process could not be identified

journal.cli.host.supervisor.recovery.initial.admission.handshake = details: a helper process did not answer in time

journal.cli.host.supervisor.recovery.launch = details: a helper process could not be started

journal.cli.host.supervisor.recovery.retry = this start could not continue.{ $line_break }{ $line_break }wait a moment, then try again.{ $line_break }{ $line_break }your memories are untouched.{ $line_break }

journal.cli.host.supervisor.supervisor.runtime.unavailable = supervisor runtime unavailable: { $error }

journal.cli.host.supervisor.supervisor.service.capture.unavailable = supervisor service capture unavailable: { $error }

journal.cli.host.supervisor.usage = usage: { $syntax }{ $line_break }{ $spaces_26 }{ $syntax_4 }{ $line_break }{ $spaces_26 }{ $syntax_5 }{ $line_break }{ $spaces_26 }{ $syntax_6 }{ $line_break }{ $spaces_26 }{ $syntax_7 }{ $line_break }

journal.cli.host.supervisor.your.journal.can.t.start.because.another.account.on.this = your journal can't start because another account on this computer is running its journal. one journal runs on a computer at a time, so yours can start once that account stops its journal or logs off.

journal.cli.host.supervisor.your.journal.can.t.start.because.port.on.this.computer = your journal can't start because port { $port } on this computer is already in use, usually by a journal running under another account. only one journal can run on this computer at a time.

journal.cli.host.thinking.chatgpt.help = usage: { $syntax }{ $line_break }{ $line_break }Manage { $term } sign-in, tokens, and model catalog.{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_14 }Sign in to { $term } via browser { $term_9 }{ $line_break }{ $spaces_4 }{ $subcommand_10 }{ $spaces_13 }Sign out and revoke active { $term } credentials{ $line_break }{ $spaces_4 }{ $subcommand_12 }{ $spaces_15 }Show current { $term } authentication status{ $line_break }{ $spaces_4 }{ $subcommand_14 }{ $spaces_15 }List available { $term } models{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }

journal.cli.host.thinking.chatgpt.models.help = usage: { $syntax }{ $line_break }{ $line_break }List available { $term } models.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_8 }Journal root{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_16 }Emit machine-readable { $term_11 }{ $line_break }

journal.cli.host.thinking.chatgpt.models.usage = usage: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.sign.in.help = usage: { $syntax }{ $line_break }{ $line_break }Sign in to { $term } via browser { $term_4 }.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_8 }Journal root{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_10 }Do not open default browser automatically{ $line_break }

journal.cli.host.thinking.chatgpt.sign.in.usage = usage: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.sign.out.help = usage: { $syntax }{ $line_break }{ $line_break }Sign out and revoke active { $term } credentials.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_8 }Journal root{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_14 }Remove stored credential file and registration{ $line_break }

journal.cli.host.thinking.chatgpt.sign.out.usage = usage: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.status.help = usage: { $syntax }{ $line_break }{ $line_break }Show current { $term } authentication status.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_8 }Journal root{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_16 }Emit machine-readable { $term_11 }{ $line_break }

journal.cli.host.thinking.chatgpt.status.usage = usage: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.usage = usage: { $syntax }{ $line_break }

journal.cli.host.thinking.credential.chatgpt.auth.server.temporarily.unavailable = { $literal_1 } auth server is temporarily unavailable (429 or 5xx)

journal.cli.host.thinking.credential.chatgpt.credential.store.busy = { $literal_1 } credential store is busy

journal.cli.host.thinking.credential.chatgpt.request.was.by.auth = { $literal_1 } request was refused by the auth server

journal.cli.host.thinking.credential.chatgpt.request.was.code = { $literal_1 } request was refused: { $code }

journal.cli.host.thinking.credential.chatgpt.server.returned.malformed.payload = { $literal_1 } server returned malformed payload (missing or invalid { $field })

journal.cli.host.thinking.credential.chatgpt.server.returned.malformed.payload.server.returned.malformed.payload = { $literal_1 } server returned malformed payload

journal.cli.host.thinking.credential.chatgpt.token.grant.was.received = { $literal_1 } token grant was received from the server but could not be saved to disk

journal.cli.host.thinking.credential.i.o.during.credential.operation = I/O error during credential operation: { $msg }

journal.cli.host.thinking.credential.network.communicating.with.chatgpt.auth = network error communicating with { $literal_1 } auth servers

journal.cli.host.thinking.credential.path.credential.storage.unreadable.or = { $path }: credential storage is unreadable or corrupted; run '{ $literal_1 }'

journal.cli.host.thinking.credential.this.chatgpt.account.isn.t = this { $literal_1 } account isn't eligible

journal.cli.host.thinking.help = usage: { $syntax }{ $line_break }{ $line_break }Select the journal thinking lane or manage { $term } authentication.{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_13 }Set the thinking lane ({ $literal }, { $literal_10 }, or { $literal_11 }){ $line_break }{ $spaces_4 }{ $subcommand_12 }{ $spaces_14 }Manage { $term } sign-in and tokens{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }

journal.cli.host.thinking.run.chatgpt.sign.in.account.mismatch.run.thinking.chatgpt = account mismatch: run '{ $literal_1 }'

journal.cli.host.thinking.run.chatgpt.sign.in.chatgpt.sign.in.failed.outcome = { $literal_1 } sign-in failed: { $outcome }

journal.cli.host.thinking.run.chatgpt.sign.in.chatgpt.sign.in.required.run = { $literal_2 } sign-in required: run '{ $literal_1 }'

journal.cli.host.thinking.run.chatgpt.sign.in.signed.in.chatgpt.successfully = signed in to { $literal_1 } successfully.

journal.cli.host.thinking.run.chatgpt.sign.out.signed.out.chatgpt = signed out of { $literal_1 }.

journal.cli.host.thinking.run.chatgpt.sign.out.signed.out.chatgpt.complete.sign = signed out of { $literal_1 }.{ $line_break }To complete sign-out, disconnect solstone in your { $literal_1 } account settings.

journal.cli.host.thinking.run.chatgpt.status.plan.usage.declined = plan usage: declined

journal.cli.host.thinking.run.chatgpt.status.status.not.signed.in = status: not signed in

journal.cli.host.thinking.run.chatgpt.status.status.signed.in = status: signed in

journal.cli.host.thinking.set.lane.help = usage: { $syntax }{ $line_break }{ $line_break }Set the thinking lane used by { $literal }.{ $line_break }{ $line_break }positional arguments:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_24 }Thinking lane to activate.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_3 }{ $argument } provider: { $literal_12 }, { $literal_13 }, { $literal_14 }, { $literal_15 }, or { $literal_16 }{ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_9 }Cloud { $argument } model name{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_8 }Journal root{ $line_break }

journal.cli.host.thinking.set.lane.usage = usage: { $syntax }{ $line_break }

journal.cli.host.thinking.usage = usage: { $syntax }{ $line_break }

journal.cli.host.top.failure = { $command_solstone_journal } top: { $error }

journal.cli.host.top.help = usage: { $syntax }{ $line_break }{ $line_break }Show interactive service, observation, task, and brain activity.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }show this help message and exit{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }enable verbose output{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }enable debug output{ $line_break }

journal.cli.host.top.usage = usage: { $syntax }{ $line_break }

journal.cli.host.up.help = usage: { $syntax }{ $line_break }{ $line_break }Start the installed journal service if it is not running, then wait until it is ready.{ $line_break }{ $line_break }options:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }show this help message and exit{ $line_break }

journal.cli.host.usage.commands = Usage:{ $line_break }{ $commands }

journal.cli.host.warm.development = A GAP is a development-layout sibling that was not built and therefore was not exercised; it is reported by name but does not fail warm.

journal.cli.host.warm.inventory = Warm checks the { $count } native binaries declared by the host maturin { $bindings } packaging leaves.

journal.cli.host.warm.linux.dynamic = On Linux, { $solstone_core_speakers_analyze } and { $solstone_core_vad_analyze } dynamically load the bundled { $onnx }, and { $solstone_core_vulkan_probe } dynamically loads the host { $vulkan } loader. Warm proves their loader resolution as well as their start-up. { $solstone_core_describe } is asserted neither static nor dynamic here; wheel validation only forbids dynamically linked { $ffmpeg }.

journal.cli.host.warm.linux.static = On Linux, { $solstone_core }, { $solstone_core_journal }, { $solstone_retention }, { $solstone_core_sol }, and { $solstone_core_depict } are statically linked musl. Warm proves that they start and reach their own code; there is no dynamic loader resolution to prove.

journal.cli.host.warm.mac = On macOS every Mach-O links { $library }. The property is non-empty but differs from Linux: warm exposes signing, quarantine, and { $dyld } failures there.

journal.cli.host.warm.platforms = Platform-restricted warm rows: { $rows }. They report NOT APPLICABLE when their applicability does not include this host and do not fail warm.

journal.cli.host.warm.repair.development = { $space_2 }repair: Build this crate in a configured development environment.

journal.cli.host.warm.repair.execution = { $space_2 }repair: Repair the reported host execution failure, then reinstall the owning host tool if needed.

journal.cli.host.warm.repair.missing = { $space_2 }repair: Reinstall or upgrade the owning host tool with the installer family that created this environment.

journal.cli.host.warm.repair.platform = { $space_2 }repair: No repair needed on this platform.

journal.cli.host.warm.row.development = GAP { $binary }: { $path } is absent beside the development executable

journal.cli.host.warm.row.executable = CANNOT LOAD { $binary }: cannot resolve current executable: { $error }

journal.cli.host.warm.row.exit = RAN { $binary }: returned exit { $code }

journal.cli.host.warm.row.library = CANNOT LOAD { $binary }: shared library { $library } could not be loaded

journal.cli.host.warm.row.missing = MISSING { $binary }: { $path } is absent beside the running executable

journal.cli.host.warm.row.no.code = CANNOT LOAD { $binary }: terminated without an exit code

journal.cli.host.warm.row.platform = NOT APPLICABLE { $binary }: Linux-only binary is not applicable on this platform

journal.cli.host.warm.row.signal = CANNOT LOAD { $binary }: terminated by signal { $signal }

journal.cli.host.warm.row.spawn = CANNOT LOAD { $binary }: failed to spawn { $path }: { $error }

journal.cli.host.warm.summary = { $quantity ->
    [one] Summary: { $ran } ran, { $missing } missing, { $cannot_load } cannot load, { $gaps } gap, { $not_applicable } not applicable.
   *[other] Summary: { $ran } ran, { $missing } missing, { $cannot_load } cannot load, { $gaps } gaps, { $not_applicable } not applicable.
    }

journal.cli.host.warm.unresolved = { $space_2 }unresolved library: { $library }

journal.cli.host.windows.service.adopted.installation.has.no.journal.path = adopted installation has no journal path

journal.cli.host.windows.service.background.service.is.not.installed = background service is not installed

journal.cli.host.windows.service.background.service.stopped = background service stopped

journal.cli.host.windows.service.background.support.for.your.journal.couldn.t.be.changed.run = background support for your journal couldn't be changed.{ $break_line }run `{ $command_solstone_journal_service_status }` to check it.{ $break_line }details: { $reason }

journal.cli.host.windows.service.background.support.for.your.journal.couldn.t.be.checked.try = background support for your journal couldn't be checked.{ $break_line }try again in a moment. if it keeps failing, include the details below in a support request.{ $break_line }details: { $reason }

journal.cli.host.windows.service.before.uninstall.deadline.is.out.of.range = before-uninstall deadline is out of range

journal.cli.host.windows.service.cannot.identify.one.running.service.task = cannot identify one running service task

journal.cli.host.windows.service.cannot.resolve.task.artifact.directory = cannot resolve task artifact directory

journal.cli.host.windows.service.clean.uninstall.requires.an.adopted.installation.identity = clean uninstall requires an adopted installation identity

journal.cli.host.windows.service.could.not.clear.prior.update.intent = could not clear prior update intent: { $error }

journal.cli.host.windows.service.could.not.clear.windows.sign.in.resume = could not clear { $literal_windows } sign-in resume: { $error }

journal.cli.host.windows.service.could.not.consume.update.intent = could not consume update intent: { $error }

journal.cli.host.windows.service.could.not.inspect.current.executable = could not inspect current executable: { $error }

journal.cli.host.windows.service.could.not.load.installation.binding = could not load installation binding: { $error }

journal.cli.host.windows.service.could.not.locate.owner.base = could not locate owner base: { $error }

journal.cli.host.windows.service.could.not.prepare.windows.sign.in.resume = could not prepare { $literal_windows } sign-in resume: { $error }

journal.cli.host.windows.service.could.not.read.update.intent.record = could not read update intent record: { $error }

journal.cli.host.windows.service.could.not.resolve.identity.root.from.executable.directory = could not resolve identity root from executable directory

journal.cli.host.windows.service.could.not.resolve.root.token = could not resolve root token: { $error }

journal.cli.host.windows.service.could.not.resolve.uninstall.root = could not resolve uninstall root

journal.cli.host.windows.service.could.not.retrieve.user.sid = could not retrieve user { $literal_sid }: { $error }

journal.cli.host.windows.service.could.not.write.update.intent.record = could not write update intent record: { $error }

journal.cli.host.windows.service.direct.worker.stop.request.direct.worker.observation = direct-worker stop request: { $termination }; direct-worker observation: { $observed }

journal.cli.host.windows.service.error.invalid.port = error: invalid port '{ $port }'

journal.cli.host.windows.service.installation.identity.census.is.unreadable = installation identity census is unreadable

journal.cli.host.windows.service.installation.provider.location.is.unavailable = installation provider location is unavailable

journal.cli.host.windows.service.installed.command.contains.invalid.text = installed command contains invalid text

journal.cli.host.windows.service.installed.task.action.did.not.match.its.readback = installed task action did not match its readback

journal.cli.host.windows.service.is.registered.expected = { $registered } is registered, expected { $expected }

journal.cli.host.windows.service.it.belongs.to.a.different.installation.of.the.journal = it belongs to a different installation of the journal

journal.cli.host.windows.service.it.is.registered.to.another.account = it is registered to another account

journal.cli.host.windows.service.it.runs.the.journal.at.expected = it runs the journal at { $registered }, expected { $journal }

journal.cli.host.windows.service.journal = journal: { $error }

journal.cli.host.windows.service.journal.path.contains.invalid.text = journal path contains invalid text

journal.cli.host.windows.service.native.process.birth.changed = native process birth changed

journal.cli.host.windows.service.native.process.birth.is.missing = native process birth is missing

journal.cli.host.windows.service.native.process.pid.is.invalid = native process PID is invalid

journal.cli.host.windows.service.native.windows.directory.is.unavailable = native { $literal_windows } directory is unavailable

journal.cli.host.windows.service.no.reason = windows gave no reason.

journal.cli.host.windows.service.scheduler.returned.no.task.run.instance = scheduler returned no task run instance

journal.cli.host.windows.service.scheduler.rpc.may.have.changed.state.re.inspection.required = { $reason }; { $observation }; scheduler RPC may have changed state; re-inspection required

journal.cli.host.windows.service.selected.journal.differs.from.the.saved.installation.binding = selected journal differs from the saved installation binding

journal.cli.host.windows.service.service.exited.abnormally.cleanup.remains.unverified = service exited abnormally; cleanup remains unverified

journal.cli.host.windows.service.service.forwarder.admission.is.invalid = service forwarder admission is invalid

journal.cli.host.windows.service.service.installation.guard.differs.from.the.saved.binding = service installation guard differs from the saved binding

journal.cli.host.windows.service.service.readiness.has.no.exact.native.identity = service readiness has no exact native identity

journal.cli.host.windows.service.service.readiness.has.no.installed.forwarder.admission = service readiness has no installed forwarder admission

journal.cli.host.windows.service.service.readiness.identity.changed = service readiness identity changed

journal.cli.host.windows.service.service.run.exited.during.admission = service run exited during admission

journal.cli.host.windows.service.service.scheduler.instance.does.not.match.its.admitted.forwarder = service scheduler instance does not match its admitted forwarder

journal.cli.host.windows.service.service.stop.deadline.elapsed.cleanup.remains.unverified = service stop deadline elapsed; cleanup remains unverified

journal.cli.host.windows.service.service.task.changed.during.run.admission = service task changed during run admission

journal.cli.host.windows.service.service.task.changed.during.start = service task changed during start

journal.cli.host.windows.service.service.task.changed.during.stop = service task changed during stop

journal.cli.host.windows.service.service.task.changed.while.recording.its.state = service task changed while recording its state

journal.cli.host.windows.service.service.task.deletion.was.not.observed = service task deletion was not observed

journal.cli.host.windows.service.service.task.differs.from.the.current.installation.binding = service task differs from the current installation binding

journal.cli.host.windows.service.service.task.instance.changed.during.stop = service task instance changed during stop

journal.cli.host.windows.service.service.task.instance.changed.while.retaining.native.handles = service task instance changed while retaining native handles

journal.cli.host.windows.service.service.task.is.absent.and.its.forwarder.cleanup.cannot.be = service task is absent and its forwarder cleanup cannot be verified

journal.cli.host.windows.service.service.task.is.not.installed = service task is not installed

journal.cli.host.windows.service.stopped.background.support.for.your.journal.starts.again.when.you = Stopped: background support for your journal starts again when you next sign in.

journal.cli.host.windows.service.stopped.background.support.for.your.journal.stays.off.until.you = Stopped: background support for your journal stays off until you start it or open the journal app.

journal.cli.host.windows.service.supervisor.readiness.not.ready = Supervisor readiness: not ready

journal.cli.host.windows.service.supervisor.readiness.ready = Supervisor readiness: ready

journal.cli.host.windows.service.systemroot.is.unavailable = SystemRoot is unavailable

journal.cli.host.windows.service.task = Task: { $task }

journal.cli.host.windows.service.task.control.deadline.elapsed.before.spawn = task control deadline elapsed before spawn

journal.cli.host.windows.service.task.control.exchange.failed = task control exchange failed: { $error }

journal.cli.host.windows.service.task.control.exchange.failed.worker.channel.closed = task control exchange failed: worker channel closed

journal.cli.host.windows.service.task.control.exchange.timed.out = task control exchange timed out

journal.cli.host.windows.service.task.control.input.exceeds.its.byte.limit = task control input exceeds its byte limit

journal.cli.host.windows.service.task.control.output.exceeds.its.byte.limit = task control output exceeds its byte limit

journal.cli.host.windows.service.task.control.spawn.failed = task control spawn failed: { $error }

journal.cli.host.windows.service.task.control.stdio.missing = task control stdio missing

journal.cli.host.windows.service.task.control.worker.exited = task control worker exited

journal.cli.host.windows.service.task.operation.returned.inconsistent.evidence = task operation returned inconsistent evidence

journal.cli.host.windows.service.task.operation.returned.invalid.json = task operation returned invalid { $literal_json }

journal.cli.host.windows.service.task.request.is.not.a.json.object = task request is not a { $literal_json } object

journal.cli.host.windows.service.task.xml.readback.missing = task XML readback missing

journal.cli.host.windows.service.the.installed.solstone.exe.facade.is.unavailable = the installed solstone.exe facade is unavailable

journal.cli.host.windows.service.the.registered.task.has.no.readable.definition = the registered task has no readable definition

journal.cli.host.windows.service.update.intent.record.is.invalid = update intent record is invalid

journal.cli.host.windows.service.windows.run.key.was.not.opened = { $literal_windows } Run key was not opened

journal.cli.host.windows.service.windows.service.action.contains.non.unicode.text = windows service action contains non-Unicode text

journal.cli.host.windows.service.windows.service.action.differs.from.the.complete.saved.binding = windows service action differs from the complete saved binding

journal.cli.host.windows.service.windows.service.binding.unavailable = windows service binding unavailable

journal.cli.host.windows.service.windows.sign.in.resume.command.exceeds.the.run.key.limit = { $literal_windows } sign-in resume command exceeds the Run key limit or has an invalid path

journal.cli.host.windows.service.windows.sign.in.resume.failed.windows.error = { $literal_windows } sign-in resume { $action } failed ({ $literal_windows } error { $code })

journal.cli.host.windows.service.windows.sign.in.resume.registry.readback.differed = { $literal_windows } sign-in resume registry readback differed

journal.cli.host.windows.service.your.journal.didn.t.report.ready.so.the.start.can = your journal didn't report ready, so the start can't be confirmed. run `{ $command_solstone_journal_service } logs` to see why

journal.cli.host.windows.service.your.journal.is.ready = your journal is ready

journal.cli.setup.parser.cwd = could not determine current directory: { $error }

journal.cli.setup.parser.path.empty = { $journal_flag } must not be empty

journal.cli.setup.parser.path.encoding = { $journal_flag } could not be resolved: path is not { $encoding }

journal.cli.setup.parser.path.resolve = { $journal_flag } could not be resolved: { $error }

journal.cli.setup.parser.port = { $port_flag } must be in { $range } (got { $value })

journal.cli.setup.parser.sign.in.encoding = argument { $sign_in_flag }: expected { $on_value } or { $off_value }

journal.cli.setup.parser.sign.in.invalid = argument { $sign_in_flag }: invalid choice: { $value } (choose from { $choices })

journal.cli.setup.parser.sign.in.repeated = argument { $sign_in_flag }: must not be repeated

journal.cli.setup.parser.timeout = argument { $timeout_flag }: invalid int value

journal.cli.setup.parser.unknown = unrecognized arguments

journal.cli.setup.parser.variant = argument { $variant_flag }: invalid choice: { $value } (choose from { $choices })

journal.cli.check.disk.error = free space at { $path } could not be verified: { $message }

journal.cli.check.disk.free = { $gb } GB free at { $path } (need 20 GB)

journal.cli.check.disk.low = the journal and local models need at least 20 GB free — { $path } has { $gb } GB

journal.cli.check.disk.overflow = free space overflow

journal.cli.check.feedback = Think this readout is wrong for your machine? We'd love a patch — { $url }

journal.cli.check.gpu.absent = no usable GPU found — the bundled local models need a hardware GPU

journal.cli.check.gpu.absent.linux = no usable GPU found — the bundled local models need a hardware GPU with at least 6 GB

journal.cli.check.gpu.cuda.integrity = CUDA runtime integrity failure

journal.cli.check.gpu.low.linux = GPU { $gpu } has { $gb } GB — the bundled local models need at least 6 GB

journal.cli.check.gpu.low.windows = GPU { $gpu } has { $gb } GB; the bundled local models need at least 6 GB

journal.cli.check.gpu.nvidia = NVIDIA GPU with { $gb } GB

journal.cli.check.gpu.nvidia.cpu = NVIDIA GPU with { $gb } GB; a model runs on your GPU; transcription runs on your CPU on this machine

journal.cli.check.gpu.nvidia.low = the NVIDIA GPU has { $gb } GB — the bundled local models need at least 6 GB

journal.cli.check.gpu.nvidia.unknown = NVIDIA GPU detected but its memory could not be read — GPU readiness is unknown

journal.cli.check.gpu.readiness.unknown = GPU readiness could not be determined: { $error }

journal.cli.check.gpu.render.permission = a GPU render node exists under { $path } but this user cannot open it — add yourself to the render group with `{ $repair }`, then log out and back in and run `{ $command }` again

journal.cli.check.gpu.unified = NVIDIA GPU with { $gb } GB (unified memory)

journal.cli.check.gpu.unified.cpu = NVIDIA GPU with { $gb } GB (unified memory); a model runs on your GPU; transcription runs on your CPU on this machine

journal.cli.check.gpu.unknown = couldn't read GPU memory. local thinking needs at least 6 GB

journal.cli.check.gpu.vulkan = Vulkan GPU { $gpu } with { $gb } GB

journal.cli.check.gpu.vulkan.cpu = Vulkan GPU { $gpu } with { $gb } GB; a model runs on your GPU; transcription runs on your CPU on this machine

journal.cli.check.gpu.vulkan.incomplete = no NVIDIA GPU found and the Vulkan probe did not complete — GPU readiness is unknown

journal.cli.check.label.ced = ced

journal.cli.check.label.disk = disk

journal.cli.check.label.gpu = gpu

journal.cli.check.label.memory = memory

journal.cli.check.label.platform = platform

journal.cli.check.label.ram = ram

journal.cli.check.label.rfdetr = rfdetr

journal.cli.check.marker.blocked = [blocked]

journal.cli.check.marker.ok = [ok]

journal.cli.check.marker.unknown = [unknown]

journal.cli.check.marker.warn = [warn]

journal.cli.check.memory.available = enough total memory, but only { $gb } GB is free right now — close some apps before the local models load

journal.cli.check.memory.eligible = { $gb } GB of memory meets the 16 GB minimum

journal.cli.check.memory.minimum = Apple Silicon needs at least 16 GB of memory for the bundled local models (this Mac has { $gb } GB)

journal.cli.check.memory.unknown = available memory could not be verified

journal.cli.check.overall.blocked = Not ready — this computer can't run the bundled local models yet.

journal.cli.check.overall.ready = Ready — install the journal next:  see { $file }

journal.cli.check.overall.warning = Mostly ready (see the warnings above) — you can install the journal:  see { $file }

journal.cli.check.platform.apple = Apple Silicon macOS (arm64)

journal.cli.check.platform.intel = Intel Macs aren't supported — use an Apple Silicon Mac, or run the journal on supported Linux.

journal.cli.check.platform.linux = Linux ({ $arch })

journal.cli.check.platform.unsupported = { $os }/{ $arch } can't run the bundled local models yet. supported platforms are apple silicon macos, x86_64/aarch64 linux and x86_64/aarch64 windows.

journal.cli.check.platform.windows = Windows ({ $arch })

journal.cli.check.ram.low = { $gb } GB of system memory is on the low side — 8 GB or more is recommended

journal.cli.check.ram.total = { $gb } GB of system memory

journal.cli.check.ram.unknown = system memory could not be verified

journal.cli.check.title = { $command } — can this computer run the journal with the bundled local models?

journal.cli.maintenance.backup.erase = backup erase: { $state } { $removed_field }{ $removed } { $shared_field }{ $shared } { $staying_field }{ $staying } { $unreached_field }{ $unreached } { $pending_field }{ $pending }

journal.cli.maintenance.backup.erase.error.detail = backup erase: error { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.erase.reason = backup erase: { $state } { $removed_field }{ $removed } { $shared_field }{ $shared } { $staying_field }{ $staying } { $unreached_field }{ $unreached } { $pending_field }{ $pending } { $reason_field }{ $reason }

journal.cli.maintenance.backup.error = backup: error { $reason_field }{ $reason }

journal.cli.maintenance.backup.error.detail = backup: error { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.ok = backup: ok { $field }{ $snapshot }

journal.cli.maintenance.backup.paused = backup: skipped. backup is paused, so nothing ran. run `{ $resume }` to start it again.

journal.cli.maintenance.backup.prune.error = backup prune: error { $reason_field }{ $reason }

journal.cli.maintenance.backup.prune.error.detail = backup prune: error { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.prune.ok = backup prune: ok

journal.cli.maintenance.backup.prune.paused = backup prune: skipped. backup is paused, so nothing ran. run `{ $resume }` to start it again.

journal.cli.maintenance.backup.prune.skipped = backup prune: skipped

journal.cli.maintenance.backup.skipped = backup: skipped

journal.cli.maintenance.backup.superseded = backup: claimed by another device; local settings were cleared. run `{ $enable }` to set up a new backup here.

journal.cli.maintenance.backup.unavailable = backup maintenance services are unavailable.

journal.cli.maintenance.backup.usage.offload = usage: { $command } { $id }{ $options }{ $line_break }{ $command } { $id }: error: unrecognized arguments: { $args }

journal.cli.maintenance.backup.usage.routine = usage: { $command } { $id }{ $options }{ $line_break }{ $command } { $id }: error: unrecognized arguments: { $args }

journal.cli.maintenance.backup.verify.error = backup verify: error { $reason_field }{ $reason }

journal.cli.maintenance.backup.verify.error.detail = backup verify: error { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.verify.ok = backup verify: ok { $field }{ $subset }

journal.cli.maintenance.backup.verify.paused = backup verify: skipped. backup is paused, so nothing ran. run `{ $resume }` to start it again.

journal.cli.maintenance.backup.verify.skipped = backup verify: skipped

journal.cli.maintenance.description.backup.erase = remove what you erased from your backups too, when you chose it.

journal.cli.maintenance.description.backup.offload = offload verified raw media after backup.

journal.cli.maintenance.description.backup.prune = apply encrypted backup retention policy.

journal.cli.maintenance.description.backup.run = run encrypted backup.

journal.cli.maintenance.description.backup.verify = verify encrypted backup read-back.

journal.cli.maintenance.description.faces.backfill = refresh profile pic suggestions from meetings, including past meetings.

journal.cli.maintenance.description.health.mark.raw = list original media ready for removal.

journal.cli.maintenance.description.health.prune.logs = prune old operational logs.

journal.cli.maintenance.description.retention.owner.voice.originals = finish deleting the original audio of recent recordings processed with "leave out other voices" on.

journal.cli.maintenance.description.speakers.backfill.labels = name the speakers, without the model, in audio that has no speaker labels yet.

journal.cli.maintenance.description.speakers.backfill.pool = catch up the voice list from audio already transcribed, then look for your voice.

journal.cli.maintenance.description.speakers.candidate.pair.suggestions = Find dense speaker candidate pairs for Suggestions.

journal.cli.maintenance.description.speakers.consolidate.pool = Consolidate dense speaker candidates.

journal.cli.maintenance.description.speakers.discover.voices = Refresh recurring voice discovery cache.

journal.cli.maintenance.description.speakers.name.variants = Find speaker name variants for Suggestions.

journal.cli.maintenance.description.trash.erase = finish trash steps a stop interrupted and erase what has waited its days.

journal.cli.maintenance.face.priority = face history priority unavailable: { $error }

journal.cli.maintenance.face.reconciliation = face reconciliation: { $error }

journal.cli.maintenance.health.config.days = { $field } must be a positive integer

journal.cli.maintenance.health.config.enabled = { $field } must be a boolean

journal.cli.maintenance.health.keep = { $routine }: your retention settings keep all original media.

journal.cli.maintenance.health.mark.entry = { $indent }{ $id }: { $reason }

journal.cli.maintenance.health.mark.refused = { $routine }: some items could not be listed:

journal.cli.maintenance.health.mark.summary = { $routine }: new items: { $new }.{ $line_break }  standing total: { $total }.

journal.cli.maintenance.health.mark.unavailable = { $routine }: could not build the list: { $error }

journal.cli.maintenance.health.prune.args.integer = usage: { $command } { $syntax }{ $line_break }{ $command }: error: argument { $flag }: invalid int value: '{ $value }'

journal.cli.maintenance.health.prune.args.missing = usage: { $command } { $syntax }{ $line_break }{ $command }: error: argument { $flag }: expected one argument

journal.cli.maintenance.health.prune.args.positive = { $routine }: { $flag } must be a positive integer

journal.cli.maintenance.health.prune.args.unexpected = usage: { $command } { $syntax }{ $line_break }{ $command }: error: unrecognized arguments: { $value }

journal.cli.maintenance.health.prune.class = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $indent }{ $class }: { $files } file, { $dirs } cache dir, { $bytes }
       *[other] { $indent }{ $class }: { $files } file, { $dirs } cache dir(s), { $bytes }
    }
   *[other] { $dirs_quantity ->
        [one] { $indent }{ $class }: { $files } file(s), { $dirs } cache dir, { $bytes }
       *[other] { $indent }{ $class }: { $files } file(s), { $dirs } cache dir(s), { $bytes }
    }
}

journal.cli.maintenance.health.prune.disabled = { $routine }: disabled

journal.cli.maintenance.health.prune.halted = { $routine }: { $message }

journal.cli.maintenance.health.prune.missing = the journal directory is unavailable

journal.cli.maintenance.health.prune.preview = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $routine }: would prune { $files } operational-log file, { $dirs } cache dir, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: would prune { $files } operational-log file, { $dirs } cache dir(s), { $bytes } { $cutoff_field }{ $cutoff }
    }
   *[other] { $dirs_quantity ->
        [one] { $routine }: would prune { $files } operational-log file(s), { $dirs } cache dir, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: would prune { $files } operational-log file(s), { $dirs } cache dir(s), { $bytes } { $cutoff_field }{ $cutoff }
    }
}

journal.cli.maintenance.health.prune.queue = a log was removed, but its search update could not be queued: { $error }

journal.cli.maintenance.health.prune.refused = { $routine }: some logs could not be pruned: { $summary }

journal.cli.maintenance.health.prune.refused.unnamed = { $routine }: some logs could not be pruned: the retention tool refused without naming an entry.

journal.cli.maintenance.health.prune.result = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $routine }: pruned { $files } operational-log file, { $dirs } cache dir, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: pruned { $files } operational-log file, { $dirs } cache dir(s), { $bytes } { $cutoff_field }{ $cutoff }
    }
   *[other] { $dirs_quantity ->
        [one] { $routine }: pruned { $files } operational-log file(s), { $dirs } cache dir, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: pruned { $files } operational-log file(s), { $dirs } cache dir(s), { $bytes } { $cutoff_field }{ $cutoff }
    }
}

journal.cli.maintenance.health.prune.unavailable = { $routine }: could not prune logs: { $error }

journal.cli.maintenance.health.prune.undateable = { $indent }error: the entry's retention date could not be determined: the entry's retention date could not be determined

journal.cli.maintenance.health.prune.unreadable = { $indent }error: { $detail }: { $detail }

journal.cli.maintenance.health.unavailable = health maintenance services are unavailable.

journal.cli.maintenance.health.usage = usage: { $command } { $id } { $syntax }{ $line_break }{ $command } { $id }: error: unrecognized arguments: { $args }

journal.cli.maintenance.help.list = usage: { $syntax }

journal.cli.maintenance.help.root = usage: { $syntax }

journal.cli.maintenance.help.run = usage: { $syntax }

journal.cli.maintenance.help.sync = usage: { $syntax }




journal.cli.maintenance.list.empty = No maintenance routines found.

journal.cli.maintenance.list.heading.description = DESCRIPTION

journal.cli.maintenance.list.heading.every = EVERY

journal.cli.maintenance.list.heading.id = ID

journal.cli.maintenance.list.heading.runtime = MAX RUNTIME

journal.cli.maintenance.list.heading.status = STATUS


journal.cli.maintenance.list.status.divergent = divergent

journal.cli.maintenance.list.status.missing = missing

journal.cli.maintenance.list.status.nothere = n/a

journal.cli.maintenance.list.status.synced = synced

journal.cli.maintenance.missing.root = usage: { $syntax }{ $line_break }{ $command }: error: the following arguments are required: { $argument }

journal.cli.maintenance.missing.run = usage: { $syntax }{ $line_break }{ $command }: error: the following arguments are required: { $argument }

journal.cli.maintenance.retention.import.owed = imported audio cleanup incomplete: { $error }

journal.cli.maintenance.retention.owed = owner voice original cleanup incomplete: { $error }

journal.cli.maintenance.retention.summary = { $routine }: deleted { $deleted }, deleted without a record { $deleted_unrecorded }, already gone { $gone }, still owed { $owed }, kept for good { $refused }, not provable { $not_provable }, nothing owed { $not_owed }, skipped while busy { $skipped }, events unreadable { $unknown }

journal.cli.maintenance.retention.usage = usage: { $id } takes no arguments.

journal.cli.maintenance.routine.unimplemented = { $count_quantity ->
    [one] maintenance routine { $id } is not yet implemented ({ $count } forwarded argument).
   *[other] maintenance routine { $id } is not yet implemented ({ $count } forwarded argument(s)).
}

journal.cli.maintenance.routine.unknown = Unknown maintenance routine: { $id }. Run `{ $command }` to see available routines.

journal.cli.maintenance.schedule.error = Error reading/updating { $path }: { $error } (cause: { $error })

journal.cli.maintenance.schedule.io = schedule I/O failed: { $detail }

journal.cli.maintenance.schedule.malformed = malformed schedules config at { $path }

journal.cli.maintenance.schedule.metadata = unknown schedule metadata keys: { $keys }

journal.cli.maintenance.schedule.shape = { $path }: schedules config must be a JSON object

journal.cli.maintenance.schedule.state.shape = schedule state at { $path } must be a JSON object

journal.cli.maintenance.speakers.error.owner.voice.look.failed = owner voice look failed: { $error }

journal.cli.maintenance.speakers.error.speaker.candidate.consolidation.failed = speaker candidate consolidation failed: { $error }

journal.cli.maintenance.speakers.error.speaker.candidate.pair.refresh.failed = speaker candidate-pair refresh failed: { $error }

journal.cli.maintenance.speakers.error.speaker.label.backfill.failed = speaker label backfill failed: { $error }

journal.cli.maintenance.speakers.error.speaker.name.variant.refresh.failed = speaker name-variant refresh failed: { $error }

journal.cli.maintenance.speakers.not.admitted = speaker discovery generation was not admitted at maintenance entry

journal.cli.maintenance.speakers.usage.backfill.labels = usage: { $command }

journal.cli.maintenance.speakers.usage.backfill.pool = usage: { $command }

journal.cli.maintenance.speakers.usage.candidate.pair.suggestions = usage: { $command }

journal.cli.maintenance.speakers.usage.consolidate.pool = usage: { $command }

journal.cli.maintenance.speakers.usage.discover.voices = usage: { $command }

journal.cli.maintenance.speakers.usage.name.variants = usage: { $command }

journal.cli.maintenance.sync.added = added: { $count }: { $ids }

journal.cli.maintenance.sync.added.empty = added: 0

journal.cli.maintenance.sync.disabled = disabled: { $count }: { $ids }

journal.cli.maintenance.sync.disabled.empty = disabled: 0

journal.cli.maintenance.sync.divergent = divergent: { $count }: { $ids }

journal.cli.maintenance.sync.divergent.empty = divergent: 0

journal.cli.maintenance.sync.removed = removed: { $count }: { $ids }

journal.cli.maintenance.sync.removed.empty = removed: 0

journal.cli.maintenance.sync.synced = synced: { $count }: { $ids }

journal.cli.maintenance.sync.synced.empty = synced: 0

journal.cli.maintenance.sync.warning.disabled = WARNING: { $id } schedule is disabled; preserved disabled

journal.cli.maintenance.sync.warning.divergent = WARNING: { $id } schedule is divergent; preserved unchanged

journal.cli.maintenance.trash.erased = trash: erased { $erased }, finished { $finished }, waiting { $waiting }, failed { $failed }

journal.cli.maintenance.trash.error = trash: { $failure }

journal.cli.maintenance.trash.usage = usage: { $id } takes no arguments.

journal.cli.maintenance.usage.face = usage: { $syntax }{ $line_break }{ $command }: error: unrecognized arguments: unexpected face reconciliation arguments

journal.cli.maintenance.usage.unknown = usage: { $syntax }{ $line_break }{ $command }: error: unrecognized arguments: { $arguments }

journal.cli.maintenance.face.generation = face backfill generation was not admitted

journal.cli.maintenance.face.log.cache = face backfill scan cache not saved: { $detail }

journal.cli.maintenance.face.log.deferred = face participation recovery deferred: { $detail }

journal.cli.maintenance.face.log.failed = face backfill failed: { $detail }

journal.cli.maintenance.speakers.discovery.helper = speaker discovery helper failed at { $stage }: { $reason }

journal.cli.maintenance.speakers.discovery.input = speaker discovery failed: { $detail }

journal.cli.host.health.timeout.malformed.frames = { $quantity ->
    [one] Timed out waiting for supervisor status (10s; dropped { $count } malformed frame){ $line_break }
   *[other] Timed out waiting for supervisor status (10s; dropped { $count } malformed frames){ $line_break }
}

journal.cli.install.status.interrupted = the download was interrupted

journal.cli.setup.host.journal.binding.changed = journal binding changed

journal.cli.setup.host.packaged.speech.unavailable = packaged speech proof unavailable

journal.cli.oauth.callback.success = signed in. you can close this tab and go back to your journal

journal.cli.oauth.callback.failure = sign-in didn't finish. go back to your journal and try again

journal.cli.oauth.oura.callback.rejected = Oura authorization rejected.

journal.cli.oauth.oura.callback.declined = Oura wasn't connected. you can close this tab.

journal.cli.oauth.oura.callback.allowed = you allowed it at Oura. you can close this tab; the terminal says when it's connected.

journal.cli.speakers.pool.refusal.progress.unreadable = voice list catch-up refused: catch-up progress file could not be read; check that your journal's awareness folder can be read

journal.cli.speakers.pool.refusal.progress.write.failed = voice list catch-up refused: catch-up progress file could not be written

journal.cli.speakers.pool.refusal.progress.write.failed.kept = voice list catch-up refused: catch-up progress file could not be written; what was added is kept

journal.cli.speakers.pool.refusal.progress.invalid = voice list catch-up refused: catch-up progress file is invalid; removing awareness/speaker_candidates_backfill.json from your journal restarts the catch-up safely

journal.cli.speakers.pool.refusal.days.unreadable = voice list catch-up refused: journal days could not be listed

journal.cli.speakers.pool.refusal.format.unsupported = voice list catch-up refused: the voice list is in a format this version can't add to

journal.cli.speakers.pool.refusal.pool.unreadable = voice list catch-up refused: voice list could not be read

journal.cli.speakers.pool.refusal.pool.write.failed = voice list catch-up refused: voice list could not be written

journal.cli.speakers.pool.refusal.pool.write.failed.kept = voice list catch-up refused: voice list could not be written; what was added is kept

journal.cli.speakers.pool.refusal.lock.timeout = voice list catch-up refused: voice list lock timed out

journal.cli.speakers.pool.refusal.lock.timeout.kept = voice list catch-up refused: voice list lock timed out; what was added is kept

journal.cli.speakers.pool.error.lock = candidate pool lock failed: { $error }

journal.cli.speakers.pool.error.read = candidate pool read failed: { $error }

journal.cli.speakers.pool.error.write = candidate pool write failed: { $error }

journal.cli.speakers.pool.error.centroid.width = candidate pool centroid width mismatch

journal.cli.speakers.owner.voice.error.identity.invalid = configured owner identity is not admitted

journal.cli.speakers.owner.voice.error.no.candidate = No candidate available

journal.cli.speakers.owner.voice.error.stale.candidate = Candidate version mismatch or stale generation

journal.cli.speakers.owner.voice.error.no.saved.voice = No owner voice is saved

journal.cli.speakers.backfill.error.journal.root = journal root admission failed: { $error }

journal.cli.speakers.backfill.error.ledger = ledger operation failed: { $error }

journal.cli.speakers.backfill.error.invalid.flags = invalid backfill flags: { $detail }

journal.cli.speakers.backfill.error.flags.mismatch = immutable backfill flags mismatch for operation { $operation_id }: expected commit={ $expected_commit } reattribute={ $expected_reattribute } accumulation={ $expected_accumulation }, got commit={ $got_commit } reattribute={ $got_reattribute } accumulation={ $got_accumulation }

journal.cli.speakers.backfill.error.not.found = backfill operation not found: { $id }

journal.cli.speakers.backfill.error.lock = backfill execution lock failed: { $error }

journal.cli.speakers.backfill.error.scan = backfill scan failed: { $error }

journal.cli.grab.status.captured = captured but not analyzed

journal.cli.grab.status.analyzed = analyzed

journal.cli.grab.status.missing.trash = analyzed; the original video is in the trash

journal.cli.grab.status.missing.policy = analyzed; you deleted the original video after your retention settings marked it

journal.cli.grab.status.missing.offload = analyzed; you deleted the original video after your backup copied it

journal.cli.grab.status.missing.owner = analyzed; you deleted the original video

journal.cli.grab.status.missing.unknown = analyzed; the original video is no longer in your journal

journal.cli.grab.missing.reason.trash = the original video is in the trash, where you can recover it in settings

journal.cli.grab.missing.reason.policy = you deleted the original video after your retention settings marked it

journal.cli.grab.missing.reason.offload = you deleted the original video after your backup copied it

journal.cli.grab.missing.reason.owner = you deleted the original video

journal.cli.grab.missing.reason.unknown = the original video is no longer in your journal

journal.cli.grab.header.day = day

journal.cli.grab.header.streams = streams

journal.cli.grab.header.segments = segments

journal.cli.grab.header.screens = screens

journal.cli.grab.header.frames.analyzed = frames_analyzed

journal.cli.grab.header.stream = stream

journal.cli.grab.header.segment = segment

journal.cli.grab.header.start = start

journal.cli.grab.header.end = end

journal.cli.grab.header.screen = screen

journal.cli.grab.header.position = position

journal.cli.grab.header.connector = connector

journal.cli.grab.header.status = status

journal.cli.grab.header.frame.id = frame_id

journal.cli.grab.header.timestamp = timestamp

journal.cli.grab.header.abs.time = abs_time

journal.cli.grab.header.primary = primary

journal.cli.grab.header.notes = notes

journal.cli.grab.next = Next: { $command }

journal.cli.grab.level4.legacy = 0 frames analyzed: file uses pre-frame_id schema

journal.cli.grab.level4.empty = No qualified frames in this screen's analysis.

journal.cli.grab.level4.footer = Inspect:    { $inspect_command }{ $line_break }Save one:   { $save_one_command }{ $line_break }Save many:  { $save_many_command }{ $line_break }{ $line_break }How extraction works:{ $line_break }  Decoding walks the video linearly from frame 0 — seeking is unsafe at the{ $line_break }  1 Hz capture rate. Cost is dominated by the highest requested frame_id, not{ $line_break }  the count. Asking for ids 7,12,23 costs the same as asking for 23 alone.{ $line_break }  Prefer batch mode when you want more than one frame from the same screen.

journal.cli.grab.level4.missing.video = Save mode unavailable: { $reason }.{ $line_break }Frame metadata above is still readable.{ $line_break }{ $line_break }Inspect: { $command }

journal.cli.grab.level5a.screen = Screen: { $value }

journal.cli.grab.level5a.jsonl = JSONL: { $value }

journal.cli.grab.level5a.video = Video: { $value }

journal.cli.grab.level5a.frame = Frame: { $value }

journal.cli.grab.level5a.time = Time: { $value }

journal.cli.grab.level5a.notes = Notes: { $value }

journal.cli.grab.level5a.footer = Save: { $save_command }{ $line_break }Batch: { $batch_command }

journal.cli.grab.saved = saved { $path }

journal.cli.grab.frame.note.error = error: { $text }

journal.cli.grab.error.too.many.tokens = grab accepts at most 5 positional tokens: day stream segment screen frame-id

journal.cli.grab.error.force.requires.out = { $force_flag } requires { $out_flag }

journal.cli.grab.error.out.requires.five.tokens = { $out_flag } requires day stream segment screen and frame-id

journal.cli.grab.error.multiple.ids.require.out = multiple frame ids require { $out_flag }

journal.cli.grab.error.captured.not.analyzed = screen { $screen } in { $segment } is captured but not analyzed

journal.cli.grab.error.frame.not.found = frame id { $frame_id } not found in { $screen } for { $segment }

journal.cli.grab.error.video.missing.with.reason = { $reason }; metadata-only access remains via: { $command }

journal.cli.grab.error.video.missing = raw video not found for screen { $screen } in { $segment }

journal.cli.grab.error.decode.failed.ids = failed to decode frame ids: { $ids }

journal.cli.grab.error.legacy.schema = screen file uses pre-frame_id schema; frame selection is unavailable

journal.cli.grab.error.invalid.frame.id = invalid frame id

journal.cli.grab.error.day.not.found = day { $day } not found

journal.cli.grab.alternatives.days = Available days (closest 5):

journal.cli.grab.error.stream.not.found = stream { $stream } not found in { $day }

journal.cli.grab.alternatives.streams = Available streams in { $day }:

journal.cli.grab.error.segment.not.found = segment { $segment } not found in { $day }/{ $stream }

journal.cli.grab.alternatives.segments = Available segments in { $day }/{ $stream }:

journal.cli.grab.error.screen.not.found = screen { $screen } not found in { $day }/{ $stream }/{ $segment }

journal.cli.grab.alternatives.screens = Available screens in { $day }/{ $stream }/{ $segment }:

journal.cli.grab.error.frame.id.positive = frame ids must be positive integers: got '{ $token }'

journal.cli.grab.error.frame.id.unique = frame ids must be unique: { $frame_id }

journal.cli.grab.error.out.extension = { $out_flag } must end in { $extensions }

journal.cli.grab.error.out.filename = output path has no valid filename

journal.cli.grab.error.out.exists = output path exists (use { $force_flag }): { $paths }

journal.cli.grab.error.save.image = failed to save image: { $error }

journal.cli.grab.error.io = failed to access { $path }: { $error }

journal.cli.grab.error.decoder.init = failed to initialize video decoder: { $error }

journal.cli.grab.error.decode.no.video = failed to decode video: no video stream

journal.cli.grab.error.decode.invalid.rgb = failed to decode video: invalid RGB frame

journal.cli.grab.error.decode.video = failed to decode video: { $error }

journal.cli.grab.error.invalid.segment.key = segment { $segment } is not a valid { $format } key

journal.cli.grab.error.timestamp.finite = timestamp must be finite

journal.cli.grab.error.timestamp.range = timestamp is out of range

journal.cli.grab.error.timestamp.invalid = invalid timestamp

journal.cli.grab.error.unsupported.level = unsupported output level

journal.cli.grab.error.unsupported.level.named = unsupported output level: { $level }

journal.cli.grab.error.invalid.level4 = invalid level 4 payload

journal.cli.grab.error.invalid.level5a = invalid level 5a payload

journal.cli.grab.error.invalid.render.payload = invalid render payload

journal.cli.top.title = solstone activity manager

journal.cli.top.malformed.events = malformed events: { $count } ({ $reason })

journal.cli.top.unknown = unknown

journal.cli.top.header.service = Service

journal.cli.top.header.task = Task

journal.cli.top.header.pid = PID

journal.cli.top.header.uptime = Uptime

journal.cli.top.header.runtime = Runtime

journal.cli.top.header.mb = MB

journal.cli.top.header.percent = %

journal.cli.top.header.last.log = Last Log

journal.cli.top.waiting.services = (waiting for services)

journal.cli.top.queued = queued: { $commands }

journal.cli.top.section.thinking.health = Thinking Health

journal.cli.top.thinking.checking = (checking)

journal.cli.top.thinking.unavailable = (status unavailable)

journal.cli.top.footer.empty = q: Quit

journal.cli.top.footer.single = r: Restart  q: Quit

journal.cli.top.footer.multi = ↑/↓: Navigate  r: Restart  q: Quit

journal.cli.top.task.status.gone = gone

journal.cli.top.task.status.ok = ok

journal.cli.top.task.status.failed = failed

journal.cli.top.section.observe = Observe

journal.cli.top.waiting.status = (waiting for status)

journal.cli.top.observe.live.badge = [LIVE]

journal.cli.top.observe.screencast = screencast { $elapsed }

journal.cli.top.observe.tmux.badge = [TMUX]

journal.cli.top.observe.tmux.captures = { $quantity ->
    [one] { $count } capture
    *[other] { $count } captures
    }

journal.cli.top.observe.idle.locked = [IDLE] locked

journal.cli.top.observe.idle = [IDLE]

journal.cli.top.observe.voice = voice { $hits }

journal.cli.top.observe.describe = describe

journal.cli.top.observe.transcribe = transcribe

journal.cli.top.observe.recent = Recent: { $segments }

journal.cli.top.section.think = Think

journal.cli.top.think.segments = seg { $completed }/{ $total }

journal.cli.top.think.agents = { $total_quantity ->
    [one] { $completed }/{ $total } agent
    *[other] { $completed }/{ $total } agents
    }

journal.cli.top.think.last.prefix = Last: { $success } ok

journal.cli.top.think.last.failed = { $failed } failed

journal.cli.top.waiting.think = (waiting for think)

journal.cli.top.crashed.heading.attention = Services needing attention:

journal.cli.top.crashed.heading.crashed = Crashed:

journal.cli.top.crashed.retrying = { $name } (Retrying; attempts: { $attempts })

journal.cli.top.crashed.crashed = { $name } (Crashed; attempts: { $attempts })

journal.cli.top.crashed.attempts = { $name } (attempts: { $attempts })

journal.cli.top.reconnecting = (reconnecting)

journal.cli.top.error.stdin.not.tty = stdin is not a terminal

journal.cli.top.error.stdout.not.tty = stdout is not a terminal

journal.cli.top.error.apply = terminal apply failed: { $detail }

journal.cli.top.error.width = terminal width failed: { $detail }

journal.cli.top.error.screen = terminal screen entry failed: { $detail }

journal.cli.top.error.terminal = terminal failure: { $detail }

journal.cli.top.error.transport = transport failure: { $detail }

journal.cli.top.error.journal.config = journal config: { $detail }

journal.cli.top.error.width.unavailable = terminal width unavailable

journal.cli.schedule.empty = No schedules configured.{ $line_break }{ $line_break }Add schedules to: { $path }

journal.cli.schedule.header.name = NAME

journal.cli.schedule.header.every = EVERY

journal.cli.schedule.header.last.run = LAST RUN

journal.cli.schedule.header.next.due = NEXT DUE

journal.cli.schedule.header.cmd = CMD

journal.cli.schedule.tag.disabled = [disabled]

journal.cli.schedule.config = Config: { $path }

journal.cli.schedule.error.entry.invalid = schedule '{ $name }' is not a valid entry

journal.cli.schedule.error.cmd.invalid = schedule '{ $name }' has invalid cmd

journal.cli.schedule.error.config.unreadable = schedules config at { $path } is unreadable

journal.cli.schedule.error.config.not.object = schedules config at { $path } must be a JSON object

journal.cli.schedule.error.config.malformed = schedules config at { $path } is malformed

journal.cli.schedule.error.state.unreadable = scheduler state at { $path } is unreadable

journal.cli.schedule.error.state.not.object = scheduler state at { $path } must be a JSON object

journal.cli.schedule.error.state.malformed = scheduler state at { $path } is malformed

journal.cli.schedule.cell.invalid = invalid

journal.cli.schedule.cell.disabled = disabled

journal.cli.schedule.cell.never = never

journal.cli.schedule.cell.now = now

journal.cli.schedule.cell.midnight = midnight

journal.cli.schedule.weekly.monday = Monday { $time }

journal.cli.schedule.weekly.tuesday = Tuesday { $time }

journal.cli.schedule.weekly.wednesday = Wednesday { $time }

journal.cli.schedule.weekly.thursday = Thursday { $time }

journal.cli.schedule.weekly.friday = Friday { $time }

journal.cli.schedule.weekly.saturday = Saturday { $time }

journal.cli.schedule.weekly.sunday = Sunday { $time }

journal.cli.think.sense.reprocess.kept.voice.only = { $quantity ->
    [one] Kept { $count } voice-only output file: a voice-only result is not reprocessed
    *[other] Kept { $count } voice-only output files: a voice-only result is not reprocessed
    }

journal.cli.think.sense.reprocess.kept.before.voice.only = { $quantity ->
    [one] Kept { $count } transcript made before "leave out other voices" was on: turn it off to reprocess that transcript
    *[other] Kept { $count } transcripts made before "leave out other voices" was on: turn it off to reprocess them
    }

journal.cli.think.sense.reprocess.dry.empty = No files to delete

journal.cli.think.sense.reprocess.dry.would.delete = { $quantity ->
    [one] Would delete { $count } output file:
    *[other] Would delete { $count } output files:
    }

journal.cli.think.sense.reprocess.deleted.count = { $quantity ->
    [one] Deleted { $count } output file
    *[other] Deleted { $count } output files
    }

journal.cli.think.sense.reprocess.deleted.file = Deleted: { $path }

journal.cli.think.sense.dry.empty = No unprocessed files found

journal.cli.think.sense.dry.would.process = { $quantity ->
    [one] Would process { $count } file ({ $breakdown }):
    *[other] Would process { $count } files ({ $breakdown }):
    }

journal.cli.install.windows.payload.missing = missing{ $line_break }  { $detail }

journal.cli.install.windows.payload.not.regular = not-regular{ $line_break }  { $detail }

journal.cli.install.windows.payload.reparse.or.symlink = reparse-or-symlink{ $line_break }  { $detail }

journal.cli.install.windows.payload.unsafe.path = unsafe-path{ $line_break }  { $detail }

journal.cli.install.windows.payload.manifest = manifest{ $line_break }  { $detail }

journal.cli.install.windows.payload.signature = signature{ $line_break }  { $detail }

journal.cli.install.windows.payload.schema = schema{ $line_break }  { $detail }

journal.cli.install.windows.payload.target = target{ $line_break }  { $detail }

journal.cli.install.windows.payload.source.commit = source-commit{ $line_break }  { $detail }

journal.cli.install.windows.payload.lock.digest = cargo-lock-sha256{ $line_break }  { $detail }

journal.cli.install.windows.payload.empty = empty{ $line_break }  { $detail }

journal.cli.install.windows.payload.unsorted.or.duplicate = unsorted-or-duplicate{ $line_break }  { $detail }

journal.cli.install.windows.payload.case.collision = case-collision{ $line_break }  { $detail }

journal.cli.install.windows.payload.digest = digest{ $line_break }  { $detail }

journal.cli.install.windows.payload.bytes = bytes{ $line_break }  { $detail }

journal.cli.install.windows.payload.missing.member = missing-member{ $line_break }  { $detail }

journal.cli.install.windows.payload.unexpected.member = unexpected-member{ $line_break }  { $detail }

journal.cli.install.pdfium.windows.executable.read = could not determine the running journal executable: { $error }

journal.cli.install.pdfium.windows.executable.parent = running journal executable has no containing directory: { $path }

journal.cli.install.pdfium.windows.executable.location = running journal executable is not in the package bin directory: { $path }

journal.cli.install.pdfium.windows.package.root = package bin directory has no package root: { $path }

journal.cli.install.pdfium.windows.payload.verify = could not verify the signed PDF app payload: { $error }

journal.cli.install.pdfium.windows.payload.refuses = signed PDF app payload refuses { $member }: { $error }

journal.cli.install.pdfium.windows.runtime = Windows PDF package verification requires a Windows runtime

journal.cli.install.rfdetr.windows.executable.read = could not determine the running journal executable: { $error }

journal.cli.install.rfdetr.windows.runtime = Windows RF-DETR package verification requires a Windows runtime

journal.cli.install.rfdetr.windows.executable.parent = running journal executable has no containing directory

journal.cli.install.rfdetr.windows.executable.location = running journal executable is not in the package bin directory: { $path }

journal.cli.install.rfdetr.windows.package.root = package bin directory has no package root

journal.cli.install.rfdetr.windows.detect.failed = rfdetr-cli detect failed

journal.cli.install.rfdetr.windows.result.limit = RF-DETR result exceeded its byte limit

journal.cli.install.rfdetr.windows.verify.failed = Windows RF-DETR package verification failed: { $error }

journal.cli.install.rfdetr.probe.failed = rf-detr engine probe failed: { $result }

journal.cli.install.rfdetr.contract.missing = compiled rf-detr delivery contract missing

journal.cli.install.rfdetr.contract.slot = compiled rf-detr delivery contract slot { $actual } does not match { $expected }

journal.cli.install.rfdetr.bundled.missing = bundled rf-detr asset missing: { $filename }; reinstall the journal package

journal.cli.install.rfdetr.asset.missing = rf-detr asset missing: { $label }

journal.cli.install.rfdetr.size.mismatch = size mismatch for { $label }: expected { $expected }, got { $actual }

journal.cli.install.rfdetr.sha256.mismatch = sha256 mismatch for { $label }

journal.cli.install.rfdetr.sidecar.missing = rf-detr sidecar missing: { $path }

journal.cli.install.rfdetr.sidecar.invalid = rf-detr sidecar invalid: { $path }: { $error }

journal.cli.install.rfdetr.sidecar.mismatch = rf-detr sidecar does not match pinned artifacts

journal.cli.install.rfdetr.binary.missing = Extracted archive did not contain { $binary }

journal.cli.install.rfdetr.member.outside = rf-detr binary was outside extraction root: { $error }

journal.cli.install.rfdetr.member.mismatch = rf-detr binary member { $actual } does not match { $expected }

journal.cli.install.fit.severity.ok = ok

journal.cli.install.fit.severity.warning = warning

journal.cli.install.fit.severity.blocked = blocked

journal.cli.install.fit.severity.unknown = unknown

journal.cli.install.fit.check.platform = platform

journal.cli.install.fit.check.ram = ram

journal.cli.install.fit.check.disk = disk

journal.cli.install.fit.check.gpu = gpu

journal.cli.install.fit.header = { $artifact } fit check: { $overall }

journal.cli.install.fit.row = [{ $severity }] { $name }: { $detail }

journal.cli.install.fit.artifact.parakeet = parakeet.cpp artifacts

journal.cli.install.fit.artifact.rfdetr = rf-detr.cpp artifacts

journal.cli.install.fit.artifact.local = local provider artifacts

journal.cli.install.fit.platform.parakeet.ok = pinned parakeet.cpp artifacts are available for { $key }

journal.cli.install.fit.platform.rfdetr.windows.ok = signed rf-detr.cpp package payload is supported for { $os }/{ $arch }

journal.cli.install.fit.platform.rfdetr.ok = pinned rf-detr.cpp artifacts are available for { $key }

journal.cli.install.fit.platform.rfdetr.blocked = rf-detr.cpp requires darwin/arm64, linux/x86_64, or linux/arm64; got { $os }/{ $arch }

journal.cli.install.fit.platform.embedded.ok = local runtime is included in the journal app

journal.cli.install.fit.platform.embedded.blocked = the journal app does not provide this local runtime

journal.cli.install.fit.platform.mlx.ok = MLX runtime is included with the journal

journal.cli.install.fit.platform.mlx.blocked = MLX requires macos on Apple silicon; got { $os }/{ $arch }

journal.cli.install.fit.platform.windows.llama.ok = signed llama-server package payload is supported for windows/{ $arch }

journal.cli.install.fit.platform.windows.llama.blocked = llama-server requires a supported windows architecture; got { $os }/{ $arch }

journal.cli.install.fit.platform.llama.ok = pinned llama-server artifact is available for { $key }

journal.cli.install.fit.platform.llama.blocked = No pinned llama-server artifact for platform { $key }

journal.cli.install.fit.ram.unknown = available memory could not be verified for { $model }

journal.cli.install.fit.ram.ok = { $available } GB available memory meets the { $required } GB requirement for { $model }

journal.cli.install.fit.ram.insufficient = insufficient RAM for { $model } (need { $required } GB available, have { $available } GB available)

journal.cli.install.fit.gpu.cuda.probe.unavailable = NVIDIA GPU probe unavailable; resolved backend is { $backend }: { $reason }

journal.cli.install.fit.gpu.cuda.memory.unknown = resolved backend is { $backend }: { $reason }; GPU memory is unknown

journal.cli.install.fit.gpu.cuda.ok = CUDA backend selected: { $reason }

journal.cli.install.fit.gpu.cuda.ok.unified = CUDA backend selected: { $reason }; GPU tiering memory uses system MemAvailable

journal.cli.install.fit.gpu.cuda.ok.cpu = CUDA backend selected: { $reason }; a model runs on your GPU; transcription runs on your CPU on this machine

journal.cli.install.fit.gpu.cuda.ok.unified.cpu = CUDA backend selected: { $reason }; GPU tiering memory uses system MemAvailable; a model runs on your GPU; transcription runs on your CPU on this machine

journal.cli.install.fit.gpu.vulkan.eligible = Vulkan GPU selected: { $name }; resolved backend is { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.eligible.cpu = Vulkan GPU selected: { $name }; resolved backend is { $backend }: { $reason }; a model runs on your GPU; transcription runs on your CPU on this machine

journal.cli.install.fit.gpu.vulkan.below.bar = GPU { $name } has { $available } GB; the bundled local models need at least 6 GB; resolved backend is { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.below.bar.cpu = GPU { $name } has { $available } GB; the bundled local models need at least 6 GB; resolved backend is { $backend }: { $reason }; a model runs on your GPU; transcription runs on your CPU on this machine

journal.cli.install.fit.gpu.vulkan.memory.unknown = GPU memory could not be read; resolved backend is { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.no.hardware = no hardware Vulkan GPU selected; resolved backend is { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.probe.incomplete = Vulkan GPU probe did not complete; resolved backend is { $backend }: { $reason }

journal.cli.install.fit.disk.requirement.downloads = known downloads

journal.cli.install.fit.disk.requirement.bundled = bundled asset installation

journal.cli.install.fit.disk.item.parakeet.model = parakeet GGUF model

journal.cli.install.fit.disk.item.parakeet.cpu = parakeet CPU server tarball

journal.cli.install.fit.disk.item.parakeet.vulkan = parakeet Vulkan server tarball

journal.cli.install.fit.disk.item.rfdetr.model = rf-detr GGUF model

journal.cli.install.fit.disk.item.rfdetr.cli = rf-detr CLI binary

journal.cli.install.fit.disk.item.mlx.model = MLX model

journal.cli.install.fit.disk.item.gguf.model = GGUF model

journal.cli.install.fit.disk.item.mmproj = mmproj

journal.cli.install.fit.disk.item.cuda.tarball = CUDA llama-server tarball

journal.cli.install.fit.disk.item.llama.tarball = llama-server tarball

journal.cli.install.fit.disk.unavailable = available disk space could not be verified at { $path }: { $error }

journal.cli.install.fit.disk.unavailable.unknown = available disk space could not be verified at { $path }: { $error }; unknown download size for { $unknown }

journal.cli.install.fit.disk.insufficient = insufficient disk space for { $requirement } (need { $required } GB, have { $available } GB free)

journal.cli.install.fit.disk.insufficient.named = insufficient disk space for { $requirement } (need { $required } GB, have { $available } GB free): { $known }

journal.cli.install.fit.disk.insufficient.unknown = insufficient disk space for { $requirement } (need { $required } GB, have { $available } GB free); unknown download size for { $unknown }

journal.cli.install.fit.disk.insufficient.named.unknown = insufficient disk space for { $requirement } (need { $required } GB, have { $available } GB free): { $known }; unknown download size for { $unknown }

journal.cli.install.fit.disk.ok = { $available } GB free for { $required } GB { $requirement }

journal.cli.install.fit.disk.ok.unknown = { $available } GB free; { $requirement } need { $required } GB; unknown download size for { $unknown }

journal.cli.install.fit.disk.overflow = available disk space overflow

journal.cli.install.fit.disk.unsupported = disk-space inspection is unsupported on this platform

journal.cli.setup.identity.io.create.home = create setup home directory: { $error }

journal.cli.setup.identity.guard.missing = identity guard fields are incomplete

journal.cli.setup.identity.guard.duplicate = duplicate identity guard { $field }

journal.cli.setup.identity.guard.invalid = invalid identity guard { $field }

journal.cli.setup.identity.foreign.artifacts = existing wrapper or service artifacts are bound to a different installation

journal.cli.setup.identity.uncertain.artifacts = artifact binding is malformed or ambiguous

journal.cli.setup.identity.home.unset = HOME is not set

journal.cli.setup.identity.path.outside = PATH resolves outside the V2 installation

journal.cli.setup.identity.windows.path.odd = Windows path has an odd byte count

journal.cli.setup.identity.adopted.no.marker = adopted record has no adoption marker

journal.cli.setup.identity.guard.mismatch = artifact guard does not match identity

journal.cli.setup.identity.guard.root.mismatch = artifact guard does not match the admitted root

journal.cli.setup.identity.guard.incomplete = artifact guard is not a complete matching identity

journal.cli.setup.identity.executable.unavailable = current executable is unavailable

journal.cli.setup.identity.wrapper.journal.unsafe = effective journal path cannot be represented safely in managed wrappers

journal.cli.setup.identity.no.bootstrap.evidence = existing artifacts have no valid bootstrap evidence

journal.cli.setup.identity.guarded.no.record = existing wrapper or service artifacts are guarded, but no matching installation record exists in identity storage

journal.cli.setup.identity.finish.setup.first = finish journal setup before changing its folder

journal.cli.setup.identity.finish.recovery.first = finish setup or recovery before changing the journal folder

journal.cli.setup.identity.generation.decimal = generation is not canonical decimal

journal.cli.setup.identity.generation.range = generation is out of range

journal.cli.setup.identity.generation.nonzero = generation must be nonzero

journal.cli.setup.identity.generation.overflow = generation overflow

journal.cli.setup.identity.not.found = installation identity does not exist

journal.cli.setup.identity.provider.app.dir = installation provider base has no application directory

journal.cli.setup.identity.provider.parent = installation provider base has no parent

journal.cli.setup.identity.root.unavailable = installation root is unavailable

journal.cli.setup.identity.legacy.needs.manifest = legacy artifacts need a valid provider-less schema-v1 manifest

journal.cli.setup.identity.legacy.manifest.invalid = legacy manifest evidence is malformed or unreadable

journal.cli.setup.identity.record.stage.malformed = malformed record stage

journal.cli.setup.identity.namespace.mismatch = namespace record does not match its root namespace

journal.cli.setup.identity.owner.root.changed = owner admission root changed during lock

journal.cli.setup.identity.windows.platform.mismatch = owner platform does not match this Windows build

journal.cli.setup.identity.prepared.uninstall = prepared identity cannot be uninstalled

journal.cli.setup.identity.record.stage.stale = stale record stage does not match its name

journal.cli.setup.identity.changed.retry = the installation changed while changing its journal folder; try again

journal.cli.setup.identity.tombstoned.no.marker = tombstoned record has no adoption marker

journal.cli.setup.prompt.existing.journal = { $path } already contains journal data; proceed? [y/N]: 

journal.cli.setup.prompt.proceed = proceed? [y/N]: 

journal.cli.setup.error.sign.in.windows.only = solstone journal setup: --initial-start-at-sign-in is only available on Windows

journal.cli.setup.error.refused.before.mutation = solstone journal setup: refused before mutation: { $error }

journal.cli.setup.error.home.dir = setup could not determine your home directory

journal.cli.setup.warning.package.receipt = solstone journal setup: warning: package receipt was not written: { $error }

journal.cli.setup.warning.manifest = warning: could not write setup manifest: { $error }

journal.cli.setup.warning.launcher.ownership = warning: could not inspect solstone launcher ownership: { $error }

journal.cli.setup.service.restart.exited = service restart exited { $code }

journal.cli.setup.plan.heading.dry.run = setup dry-run:

journal.cli.setup.plan.heading.explain = setup plan:

journal.cli.setup.plan.mode = { $indent }mode: { $mode }

journal.cli.setup.plan.journal = { $indent }journal: { $path } ({ $source })

journal.cli.setup.plan.port = { $indent }port: { $value } ({ $source })

journal.cli.setup.plan.variant = { $indent }variant: { $value } ({ $source })

journal.cli.setup.plan.step.timeout = { $indent }step_timeout_seconds: { $value } ({ $source })

journal.cli.setup.plan.model.download.default = { $indent }model download: no setup time limit (default)

journal.cli.setup.plan.source.checkout.true = { $indent }source checkout: True

journal.cli.setup.plan.source.checkout.false = { $indent }source checkout: False

journal.cli.setup.plan.step.header = [step { $index }/{ $total }] { $name }

journal.cli.setup.plan.identity.admission = identity admission: would run before mutating setup steps

journal.cli.setup.plan.doctor = would run: { $binary } journal doctor --readiness

journal.cli.setup.plan.write.config = would write: { $path }

journal.cli.setup.plan.install.models = would run: { $binary } journal install-models --variant { $variant }

journal.cli.setup.plan.skills.user = would run: { $binary } skills install --agent all

journal.cli.setup.plan.skills.journal = would run: { $binary } skills install --project { $journal } --agent all

journal.cli.setup.plan.wrapper.windows = would skip POSIX wrapper provisioning; the Windows package exposes the commands directly

journal.cli.setup.plan.wrapper.unix = would provision managed solstone and journal wrappers in-process

journal.cli.setup.plan.service = would install and start the journal service

journal.cli.setup.plan.brain = would set the local provider lane if this computer can run the local model, and bootstrap it if nothing blocks it

journal.cli.setup.narrate.sol.keeps.journal = solstone on this Mac already keeps this journal, so setup did not install a background launcher.{ $line_break }Run solstone journal doctor if something looks wrong.

journal.cli.setup.narrate.step.running.command = [step { $index }/{ $total }] running { $name }: { $command }

journal.cli.setup.narrate.step.running = [step { $index }/{ $total }] running { $name }...

journal.cli.setup.narrate.model.recovery.installer = model installation did not finish. run { $journal } install-models --variant { $variant }.

journal.cli.setup.narrate.model.recovery.direct = model installation did not finish. run { $journal } install-models --variant { $variant }, then { $journal } setup.

journal.cli.setup.narrate.prior.clean.force = solstone journal setup last ran cleanly on { $completed_at }; re-running all steps (--force).

journal.cli.setup.narrate.prior.clean.verify = solstone journal setup last ran cleanly on { $completed_at }; verifying current state.

journal.cli.setup.narrate.prior.force.hint = Use --force to re-run all steps unconditionally.

journal.cli.setup.narrate.prior.incomplete.header = solstone journal setup last run on { $started_at } left these steps incomplete:

journal.cli.setup.narrate.prior.incomplete.item = { $indent }- { $name } (failed)

journal.cli.setup.narrate.prior.incomplete.hint = Re-running will verify state and re-run incomplete steps.

journal.cli.setup.narrate.step.skipped = [step { $index }/{ $total }] skipped { $name }: { $reason }

journal.cli.setup.narrate.step.failed = solstone journal setup: { $name } failed: { $message }

journal.cli.setup.narrate.complete.set.up = solstone is set up.

journal.cli.setup.narrate.complete.installed = solstone is installed.

journal.cli.setup.narrate.complete.counts = { $skipped } of { $total } steps already done; ran { $ran }

journal.cli.setup.narrate.artifacts.header = artifacts:

journal.cli.setup.narrate.artifacts.none = { $indent }none

journal.cli.setup.narrate.close.skip.service.unix = once your journal is running, open it with: solstone journal open

journal.cli.setup.narrate.close.skip.service.url = once your journal is running, open it at { $url }

journal.cli.setup.narrate.close.open.unix = open your journal: solstone journal open

journal.cli.setup.narrate.close.open.url = open your journal: { $url }

journal.cli.setup.narrate.close.finish.unix = finish setup: solstone journal open /init

journal.cli.setup.narrate.close.finish.url = finish setup: { $url }/init

journal.cli.setup.wrapper.retired.note = removed the older version's command-line tools; their backups are in ~/.local/share/solstone/setup-backups

journal.cli.setup.wrapper.skip.path = left shell login files unchanged because --skip-path was selected

journal.cli.setup.path.manual = path: could not auto-add ~/.local/bin to PATH — add this line to your shell rc manually: export PATH="$HOME/.local/bin:$PATH"

journal.cli.setup.path.already = path: ~/.local/bin already on PATH

journal.cli.setup.path.added.restart = path: added ~/.local/bin to shell PATH — restart your shell or run 'exec $SHELL -l' to pick it up

journal.cli.setup.path.added = path: added ~/.local/bin to shell PATH

journal.cli.setup.path.windows = Windows packages expose solstone directly; no POSIX PATH wrapper was installed.

journal.cli.setup.clean.config.io = could not read config: { $error }

journal.cli.setup.clean.config.invalid.toml = config is not valid TOML

journal.cli.setup.clean.config.missing.journal = config has no journal key

journal.cli.setup.clean.config.not.string = config journal value is not a string

journal.cli.setup.clean.config.not.absolute = config journal path is not absolute

journal.cli.setup.clean.preflight.failed = clean uninstall preflight failed: { $error }

journal.cli.setup.clean.refused = clean uninstall refused: { $reason }

journal.cli.setup.clean.no.binding = clean uninstall refused: no adopted installation identity

journal.cli.setup.clean.journal.line = journal: { $path }

journal.cli.setup.clean.no.owned.path = no owned path

journal.cli.setup.clean.readmit.failed = could not admit the installation again after removing its service: { $error }

journal.cli.setup.clean.changed.during.service = the installation changed while its service was being removed

journal.cli.setup.clean.step.running = [step { $step }/{ $total }] running { $name } uninstall...

journal.cli.setup.clean.preflight.overlap = identity storage overlaps a protected journal

journal.cli.setup.clean.preflight.census.error = installation identity census is unreadable: { $error }

journal.cli.setup.clean.preflight.census.unreadable = installation identity census is unreadable

journal.cli.setup.clean.preflight.config.mismatch = config journal does not match the installation identity

journal.cli.setup.clean.preflight.prepared = installation identity is prepared

journal.cli.setup.clean.refusal.jsonl = JSONL output is not supported for --clean-uninstall in this version.

journal.cli.setup.clean.refusal.incompatible = --clean-uninstall cannot be combined with { $flags }

journal.cli.setup.clean.confirm.header = solstone journal setup --clean-uninstall will remove these runtime artifacts:

journal.cli.setup.clean.confirm.service.unavailable = { $indent }service location unavailable: { $error }

journal.cli.setup.clean.confirm.marker.present = present

journal.cli.setup.clean.confirm.marker.absent = absent

journal.cli.setup.clean.confirm.marker.retain = retain

journal.cli.setup.clean.confirm.service = { $indent }[{ $marker }] service: { $path }

journal.cli.setup.clean.confirm.wrapper = { $indent }[{ $marker }] wrapper: { $path }

journal.cli.setup.clean.confirm.config = { $indent }[{ $marker }] config: { $path }

journal.cli.setup.clean.confirm.manifest = { $indent }[{ $marker }] journal manifest: { $path }

journal.cli.setup.clean.confirm.keep.header = will not remove:

journal.cli.setup.clean.confirm.keep.journal = { $indent }- journal directory: { $path }

journal.cli.setup.clean.confirm.keep.macos.app = { $indent }- /Applications/solstone.app

journal.cli.setup.clean.confirm.keep.macos.support = { $indent }- ~/Library/Application Support/solstone/

journal.cli.setup.clean.confirm.keep.macos.permissions = { $indent }- macOS microphone or screen recording permissions

journal.cli.setup.clean.confirm.keep.python = { $indent }- a leftover pip, uv or pipx journal install

journal.cli.setup.clean.child.exited = service uninstall exited { $code }

journal.cli.setup.clean.child.exited.detail = service uninstall exited { $code }: { $detail }

journal.cli.setup.clean.child.timed.out = service uninstall exited { $code } (timed out)

journal.cli.setup.clean.child.timed.out.detail = service uninstall exited { $code } (timed out): { $detail }

journal.cli.setup.clean.reason.preserved.mark = preserved: { $mark }

journal.cli.setup.clean.reason.service.guard.mismatch = service or wrapper guard does not match this installation, not removing

journal.cli.setup.clean.reason.wrapper.guard.mismatch = wrapper guard does not match this installation, not removing

journal.cli.setup.clean.reason.wrapper.worktree = refusing to act from a git worktree

journal.cli.setup.clean.reason.wrapper.cross.repo = alias points at { $target }, not removing

journal.cli.setup.clean.reason.wrapper.dangling = alias is dangling (target { $target } missing), not removing

journal.cli.setup.clean.reason.wrapper.unknown = unknown

journal.cli.setup.clean.reason.wrapper.foreign = alias is not a managed symlink, not removing

journal.cli.setup.clean.reason.wrapper.unexpected = unexpected alias state: { $state }

journal.cli.setup.clean.reason.preserved.symlink = preserved symlink

journal.cli.setup.clean.reason.preserved.non.dir = preserved non-directory target

journal.cli.setup.clean.reason.manifest.preserved = journal manifest is preserved

journal.cli.setup.clean.reason.skill.preserved = your own skill content is preserved

journal.cli.setup.clean.reason.skill.foreign = symlink target is foreign

journal.cli.setup.clean.reason.skipped.after.failure = step was not run after an earlier failure

journal.cli.setup.clean.reason.no.owned.path.known = no owned path is known

journal.cli.setup.clean.reason.no.owned.path.platform = no owned path on this platform

journal.cli.setup.clean.outcome.already.complete = clean uninstall is already complete

journal.cli.setup.clean.outcome.stopped.failed = clean uninstall stopped after a failed step

journal.cli.setup.clean.outcome.service.unavailable = service location unavailable: { $error }

journal.cli.setup.clean.outcome.no.eligible = no eligible setup files to remove

journal.cli.setup.clean.outcome.not.tty = not a tty; rerun with --yes to proceed non-interactively (cancelled)

journal.cli.setup.clean.outcome.cancelled = cancelled

journal.cli.setup.clean.outcome.complete = clean uninstall complete: { $removed } removed, { $already_absent } already-absent, { $preserved } preserved, { $skipped } skipped, { $failed } failed

journal.cli.importer.count.recording = { $quantity ->
    [one] 1 recording
   *[other] { $count } recordings
}

journal.cli.importer.count.day = { $quantity ->
    [one] 1 day
   *[other] { $count } days
}

journal.cli.importer.count.entity = { $quantity ->
    [one] 1 entity
   *[other] { $count } entities
}

journal.cli.importer.count.facet = { $quantity ->
    [one] 1 facet
   *[other] { $count } facets
}

journal.cli.importer.count.warning = { $quantity ->
    [one] 1 warning
   *[other] { $count } warnings
}

journal.cli.importer.count.message = { $quantity ->
    [one] 1 message
   *[other] { $count } messages
}

journal.cli.importer.count.conversation = { $quantity ->
    [one] 1 conversation
   *[other] { $count } conversations
}

journal.cli.importer.count.event = { $quantity ->
    [one] 1 event
   *[other] { $count } events
}

journal.cli.importer.count.attendee = { $quantity ->
    [one] 1 unique attendee
   *[other] { $count } unique attendees
}

journal.cli.importer.count.calendar.event = { $quantity ->
    [one] 1 calendar event
   *[other] { $count } calendar events
}

journal.cli.importer.count.pdf.document = { $quantity ->
    [one] 1 PDF document
   *[other] { $count } PDF documents
}

journal.cli.importer.count.total.page = { $quantity ->
    [one] 1 total page
   *[other] { $count } total pages
}

journal.cli.importer.count.segment = { $quantity ->
    [one] 1 segment
   *[other] { $count } segments
}

journal.cli.importer.count.member = { $quantity ->
    [one] 1 member
   *[other] { $count } members
}

journal.cli.importer.count.source.byte = { $quantity ->
    [one] 1 source byte
   *[other] { $count } source bytes
}

journal.cli.importer.count.tile = { $quantity ->
    [one] 1 tile
   *[other] { $count } tiles
}

journal.cli.importer.count.part = { $quantity ->
    [one] 1 part
   *[other] { $count } parts
}

journal.cli.importer.count.workout = { $quantity ->
    [one] 1 workout
   *[other] { $count } workouts
}

journal.cli.importer.count.night = { $quantity ->
    [one] 1 night
   *[other] { $count } nights
}

journal.cli.importer.count.mindful.session = { $quantity ->
    [one] 1 mindful session
   *[other] { $count } mindful sessions
}

journal.cli.importer.count.reading = { $quantity ->
    [one] 1 reading
   *[other] { $count } readings
}

journal.cli.importer.count.daily.total = { $quantity ->
    [one] 1 daily total
   *[other] { $count } daily totals
}

journal.cli.importer.count.logged.entry = { $quantity ->
    [one] 1 logged entry
   *[other] { $count } logged entries
}

journal.cli.importer.desktop.exe.no.parent = current executable has no parent

journal.cli.importer.desktop.missing.sibling = missing sibling executable { $path }

journal.cli.importer.size.recordings.across = { $recordings } across { $days }

journal.cli.importer.size.skipped = { $count } skipped

journal.cli.importer.size.listed.moves = { $quantity ->
    [one] 1 workout already listed on the activities page would move to this import's facet from { $from }
   *[other] { $count } workouts already listed on the activities page would move to this import's facet from { $from }
}

journal.cli.importer.force.day.notice = notice: this file dates to { $fresh }, and a previous import used { $stored }. this --force import will use { $planned }. existing recordings are not moved.

journal.cli.importer.embedded.body.refusal = Apple Health exports and Oura data are brought in by a journal on a computer, so nothing was brought in.

journal.cli.importer.notes.folder.retired = importing a folder of notes is gone; an Obsidian vault comes into your journal when you turn it on in the solstone app on the computer where the vault lives

journal.cli.importer.help.usage = usage: solstone journal importer [-h] [--timestamp TIMESTAMP] [--facet FACET] [--no-facet]

journal.cli.importer.help.summary = Import a media file into the journal

journal.cli.importer.help.positional.header = positional arguments:

journal.cli.importer.help.positional.media = { $indent }media                 Path to audio or text file

journal.cli.importer.help.options.header = options:

journal.cli.importer.help.opt.help = { $indent }-h, --help            show this help message and exit

journal.cli.importer.help.opt.timestamp = { $indent }Generic .txt/.md use this, or --auto to adopt a detected timestamp (--timestamp YYYYMMDD_HHMMSS, or a date and time with its offset, such as 2025-11-02T01:30:00-07:00)

journal.cli.importer.help.opt.facet.line1 = { $indent }--facet FACET         File this audio or text file, or this body import, under FACET,

journal.cli.importer.help.opt.facet.line2 = { $indent }one of your unmuted facets, instead of letting your journal choose

journal.cli.importer.help.opt.facet.line3 = { $indent }(for a body import: your remembered facet, or none)

journal.cli.importer.help.opt.no.facet.line1 = { $indent }--no-facet            File this body import under no facet, whatever the remembered

journal.cli.importer.help.opt.no.facet.line2 = { $indent }default is

journal.cli.importer.help.opt.source = { $indent }--source SOURCE       Required for classified formats (ics, chat export, image, PDF)

journal.cli.importer.help.opt.force = { $indent }--force               Force re-import; body sources create a separate import

journal.cli.importer.help.opt.auto = { $indent }--auto [AUTO]         Adopt the detected timestamp; AUTO is guidance, not the file path

journal.cli.importer.help.opt.dry.run = { $indent }--dry-run             Show what would be imported without writing to the journal

journal.cli.importer.help.opt.confirm.body = { $indent }--confirm-body-save   Confirm this run may save sensitive body importer output

journal.cli.importer.help.opt.deterministic.line1 = { $indent }--deterministic-only  When no date can be read from the file, skip it (exit 0)

journal.cli.importer.help.opt.deterministic.line2 = { $indent }instead of failing

journal.cli.importer.help.opt.backends = { $indent }--backends            List syncable importer backends

journal.cli.importer.help.opt.save = { $indent }--save                With --sync: download and import new files (default is dry-run)

journal.cli.importer.help.opt.list.importers = { $indent }--list-importers      List available file importers

journal.cli.importer.help.opt.audio.archive.line1 = { $indent }--audio-archive PATH  Import a folder of Ogg Opus recordings or a JSON manifest

journal.cli.importer.help.opt.audio.archive.line2 = { $indent }Folder: Limitless filenames and --source-zone are required.

journal.cli.importer.help.opt.audio.archive.line3 = { $indent }Manifest: { $example }

journal.cli.importer.help.opt.audio.archive.line4 = { $indent }Or use local_civil and source_zone for a member's time.

journal.cli.importer.help.opt.audio.archive.line5 = { $indent }Paths are absolute or relative to the manifest.

journal.cli.importer.help.opt.audio.archive.line6 = { $indent }Explicit manifest times take precedence over filenames.

journal.cli.importer.help.opt.audio.archive.line7 = { $indent }Supports single-stream mono/stereo Ogg Opus; other containers are left out.

journal.cli.importer.help.opt.audio.archive.line8 = { $indent }Shows size before saving; --dry-run saves nothing.

journal.cli.importer.help.opt.source.zone = { $indent }--source-zone IANA    Time zone for folder audio archives or civil times (e.g. America/New_York)

journal.cli.importer.help.opt.resume = { $indent }--resume IMPORT_ID    Fill unsaved parts of an audio archive's frozen plan

journal.cli.importer.help.opt.archive.status = { $indent }Print compact status for an audio archive run

journal.cli.importer.help.opt.archive.members = { $indent }List members for an audio archive run (use with --offset and --limit)

journal.cli.importer.help.opt.offset = { $indent }--offset OFFSET       Starting member offset for member listing

journal.cli.importer.help.opt.limit = { $indent }--limit LIMIT         Maximum members to list (default 100, max 1000)

journal.cli.importer.help.opt.json.line1 = { $indent }--json                Output results as JSON; archive saves emit progress objects

journal.cli.importer.help.opt.json.line2 = { $indent }and a final result, one JSON object per line.

journal.cli.importer.help.opt.verbose = { $indent }-v, --verbose         Enable verbose output

journal.cli.importer.help.opt.debug = { $indent }-d, --debug            Enable debug logging

journal.cli.importer.list.header = File importers:

journal.cli.importer.list.desc.ics = Import events from ICS calendar files or a Google Calendar export ZIP

journal.cli.importer.list.desc.claude = Import conversations from a Claude chat export

journal.cli.importer.list.desc.chatgpt = Import conversations from a ChatGPT export

journal.cli.importer.list.desc.gemini = Import conversations from a Google Takeout Gemini or Bard export

journal.cli.importer.list.desc.document = Import PDF documents with worker-backed text and raster extraction

journal.cli.importer.list.desc.image = Import a single image and describe its contents with vision

journal.cli.importer.list.desc.journal.archive = Merge an exported journal archive into the current journal

journal.cli.importer.list.desc.apple.health = Preview Apple Health export.xml data without writing to the journal

journal.cli.importer.list.desc.oura = Preview Oura API v2 JSON documents (synthetic fixtures; save path is a later, gated phase)

journal.cli.importer.list.desc.strava = Workouts from a Strava download (only its activities.csv is read)

journal.cli.importer.list.desc.evernote = Notes from an Evernote export

journal.cli.importer.backends.header = Syncable backends:

journal.cli.importer.skipped.already = Import skipped: this file was already imported; use --force to import it again

journal.cli.importer.skipped.no.date = Import skipped: no date could be read from the file; name one with --timestamp YYYYMMDD_HHMMSS

journal.cli.importer.skipped.confirm.date = Import skipped: the file's date needs confirming; rerun with --timestamp or --auto

journal.cli.importer.timestamp.confirm = detected timestamp { $timestamp }; rerun with --timestamp { $timestamp } or --auto

journal.cli.importer.generic.text.complete = Generic text import complete: segments={ $segments }

journal.cli.importer.generic.text.untimed.line1 = no turn times found in this file, so it landed as one segment at the import's start time, each line as written.

journal.cli.importer.generic.text.untimed.line2 = if your transcripts mark times in a way the journal doesn't read yet, ask us to add it: { $url }

journal.cli.importer.sync.complete = { $backend } sync complete: saved={ $saved } errors={ $errors }

journal.cli.importer.plaud.preview.complete = Plaud sync preview complete: files={ $files }

journal.cli.importer.source.preview = { $source } preview: date_range={ $start }..{ $end } items={ $items } entities={ $entities } summary={ $summary }

journal.cli.importer.source.complete = { $source } import complete: entries_written={ $entries } files_created={ $files } errors={ $errors } summary={ $summary }

journal.cli.importer.source.failed = { $source } import failed: { $detail }

journal.cli.importer.source.archive.complete = { $source } import complete: segments_copied={ $segments_copied } imports_copied={ $imports_copied } entities_created={ $entities_created } entities_merged={ $entities_merged } facets_created={ $facets_created } facets_merged={ $facets_merged }

journal.cli.importer.body.zones.under.hour = less than an hour of { $source } may show at the wrong time: it was saved in a different time zone from the one this journal holds for { $source }.

journal.cli.importer.body.zones.hours = { $quantity ->
    [one] 1 hour of { $source } may show at the wrong time: it was saved in a different time zone from the one this journal holds for { $source }.
   *[other] { $count } hours of { $source } may show at the wrong time: they were saved in a different time zone from the one this journal holds for { $source }.
}

journal.cli.importer.body.days.hidden = { $quantity ->
    [one] 1 day of body data was hidden in the archive, so it's hidden in this journal too.
   *[other] { $count } days of body data were hidden in the archive, so they're hidden in this journal too.
}

journal.cli.importer.source.archive.already.present = { $source } import did not merge anything: archive content is already present

journal.cli.importer.source.archive.incomplete = { $source } import incomplete: { $detail }

journal.cli.importer.plaud.begin.invalid.redirect = invalid Plaud callback redirect URI

journal.cli.importer.plaud.begin.loopback.bind = could not bind local callback listener

journal.cli.importer.plaud.begin.random.failed = failed to generate sign-in identifier

journal.cli.importer.plaud.failure.token.refused = sign-in expired or was revoked; sign in again in settings

journal.cli.importer.plaud.failure.catalogue = catalogue failed

journal.cli.importer.plaud.failure.manifest = import matching failed

journal.cli.importer.plaud.failure.temporary.url = failed to get download URL

journal.cli.importer.plaud.failure.audio.not.ready = audio download link is not ready yet; will try again on next sync

journal.cli.importer.plaud.failure.download = download failed

journal.cli.importer.plaud.failure.pipeline = import failed

journal.cli.importer.plaud.sync.missing.credential = Plaud credential is not configured

journal.cli.importer.plaud.sync.operation = Plaud { $detail }

journal.cli.importer.plaud.sync.state.failed = Plaud sync state failed: { $message }

journal.cli.importer.plaud.connect.detail.denied = sign-in was not approved

journal.cli.importer.plaud.connect.detail.callback.invalid = the sign-in address was invalid

journal.cli.importer.plaud.connect.detail.exchange.failed = couldn't finish signing in

journal.cli.importer.plaud.connect.detail.cancelled = sign-in cancelled

journal.cli.importer.plaud.connect.detail.expired = the sign-in attempt timed out

journal.cli.importer.plaud.connect.detail.busy = another sign-in or sync is already running

journal.cli.importer.plaud.connect.detail.storage = couldn't save the sign-in in this journal

journal.cli.importer.oura.app.needed = this journal isn't connected to an app you've registered with Oura yet. register one, then run `solstone journal importer --connect oura` at a terminal and paste in its client ID and client secret. how: { $how }

journal.cli.importer.oura.app.prompt.intro = this journal isn't connected to an app you've registered with Oura yet. to connect one:

journal.cli.importer.oura.app.prompt.step1 = { $indent }1. sign in at https://cloud.ouraring.com/oauth/applications and create an app

journal.cli.importer.oura.app.prompt.step2 = { $indent }2. add the redirect URI http://localhost:8765/callback

journal.cli.importer.oura.app.prompt.step3 = { $indent }3. paste the app's client ID and client secret below

journal.cli.importer.oura.app.prompt.note = they're kept in this journal's settings and used only to connect to Oura. nothing is saved until Oura says yes. how: { $how }

journal.cli.importer.oura.app.prompt.client.id = client ID:

journal.cli.importer.oura.app.prompt.client.secret = client secret:

journal.cli.importer.oura.connect.failed.timeout = Oura's sign-in didn't finish within five minutes, so nothing was saved. run it again to retry.

journal.cli.importer.oura.connect.failed.declined = Oura didn't connect, so nothing was saved.

journal.cli.importer.oura.connect.failed.exchange = Oura didn't accept the connection, so nothing was saved. check the client secret on your app's page at Oura and run it again.

journal.cli.importer.oura.connect.failed.callback = Oura's sign-in comes back to this computer on port 8765, and something else is using it. close that and run it again.

journal.cli.importer.oura.connect.failed.browser = a browser couldn't be opened here. run this at the journal's computer, not over a remote session.

journal.cli.importer.body.facet.no.means.strava = if you say no, new workouts aren't listed on the activities page, no people or things come from them, and agents limited to some facets won't see them. search and the body app still show them.

journal.cli.importer.body.facet.no.means.body = if you say no, new workouts and sleep aren't listed on the activities page, and agents limited to some facets won't see them. search and the body app still show them.

journal.cli.importer.body.facet.ask.existing = file your workouts, sleep and other body data under the facet you named { $facet }?

journal.cli.importer.body.facet.ask.make.body = make a body facet for your workouts, sleep and other body data?

journal.cli.importer.body.facet.ask.unmute = your body facet is muted. turn it back on for your workouts, sleep and other body data?

journal.cli.importer.body.facet.moves.strava = { $quantity ->
    [one] a yes moves 1 workout already filed, from { $from }.
   *[other] a yes moves { $count } workouts already filed, from { $from }.
}

journal.cli.importer.body.facet.moves.body = { $quantity ->
    [one] a yes can move up to 1 record already listed on the activities page, from { $from }.
   *[other] a yes can move up to { $count } records already listed on the activities page, from { $from }.
}

journal.cli.importer.body.facet.prompt.yn = { $question } [y/n]

journal.cli.importer.body.facet.save.failed = your answer couldn't be saved: { $error }

journal.cli.importer.body.apple.whole = an Apple Health import always reads the whole export, so it doesn't take --date-from, --date-to or --force. nothing was saved; run it again without them.

journal.cli.importer.body.apple.already.ran = this import has already run, so nothing was brought in. start a new one from the Apple Health card.

journal.cli.importer.body.apple.card.only = --import-id is only for saving an Apple Health export from its card, so nothing was brought in.

journal.cli.importer.body.apple.unknown.import = --import-id names no import, so nothing was brought in.

journal.cli.importer.body.apple.wrong.export = that import doesn't hold this export, so nothing was brought in.

journal.cli.importer.body.apple.reads.workouts = workouts

journal.cli.importer.body.apple.reads.sleep = sleep

journal.cli.importer.body.apple.reads.mindful = mindful time

journal.cli.importer.body.apple.reads.readings = heart rate and other readings

journal.cli.importer.body.apple.reads.daily = daily totals like steps and distance

journal.cli.importer.body.apple.reads.logged = logged entries like food and symptoms

journal.cli.importer.body.apple.preview.intro = this looks like an Apple Health export. saving it reads { $head }, and { $tail }.

journal.cli.importer.body.apple.preview.span = it runs from { $first } to { $last }: { $workouts }, { $nights } put together from sleep stages, { $mindful }, { $readings }, { $daily } and { $logged }.

journal.cli.importer.body.apple.preview.moves = { $quantity ->
    [one] saying yes can move up to 1 record already listed under another facet.
   *[other] saying yes can move up to { $count } records already listed under another facet.
}

journal.cli.importer.body.oura.bad.date = { $flag } takes a date like 2026-01-31, and { $text } isn't one. nothing was read.

journal.cli.importer.body.oura.window.zero = --window-days needs at least one day. nothing was read.

journal.cli.importer.body.oura.window.conflict = use --window-days or dates, not both. nothing was read.

journal.cli.importer.body.oura.date.to.needs.from = --date-to needs a --date-from to go with it. nothing was read.

journal.cli.importer.body.oura.range.future = that range goes past today, and Oura has nothing there yet. nothing was read.

journal.cli.importer.error.preview.failed = could not preview import: { $error }

journal.cli.importer.error.stopped = import stopped

journal.cli.importer.connect.unknown.backend = Unknown connect backend: { $backend }

journal.cli.importer.connect.available = Connectable backends: oura, plaud

journal.cli.importer.connect.oura.both.required = Oura needs both the client ID and the client secret, so nothing was saved.

journal.cli.importer.connect.oura.done = Oura is connected, and the connection is saved in this journal's settings.

journal.cli.importer.connect.oura.scopes = permissions asked for: { $scopes }

journal.cli.importer.sync.unknown.backend = Unknown sync backend: { $backend }

journal.cli.importer.sync.available = Available backends: { $backends }

journal.cli.importer.error.zone.changed.preview = the time zone changed after preview; preview again before importing

journal.cli.importer.error.no.facet.conflict = argument --no-facet: not allowed with argument --facet

journal.cli.importer.error.facets.unreadable = could not read your facets, so nothing was imported: { $error }

journal.cli.importer.error.facet.not.unmuted = "{ $facet }" is not one of your unmuted facets, so nothing was imported. your unmuted facets: { $enabled }

journal.cli.importer.error.facet.scope = --facet only applies to an audio file, a plain text file or a body import, so nothing was imported; import this one without it

journal.cli.importer.error.no.facet.scope = --no-facet only applies to a body import, so nothing was imported; import this one without it

journal.cli.importer.error.timestamp.format.offset = timestamp must be YYYYMMDD_HHMMSS, or a date and time with its offset, such as 2025-11-02T01:30:00-07:00

journal.cli.importer.error.timestamp.format = timestamp must be YYYYMMDD_HHMMSS format

journal.cli.importer.audio.failed = audio import failed: { $error }

journal.cli.importer.audio.runtime.failed = audio import runtime failed: { $error }

journal.cli.importer.audio.manifest.failed = audio import saved to your journal, but this file couldn't be marked as imported, so importing it again would add it a second time: { $error }

journal.cli.importer.audio.processing.failed = audio import processing failed: { $segments }

journal.cli.importer.audio.processing.stalled = audio import still processing when the importer stopped waiting: { $segments }

journal.cli.importer.audio.complete = Generic audio import complete: { $outcome }

journal.cli.importer.audio.dropped.range = { $start } to { $end }

journal.cli.importer.audio.dropped.notice = part of this recording couldn't be imported: { $ranges } into the recording. the other parts are in your journal, and importing the file again would add them a second time.

journal.cli.importer.text.failed = text import failed: { $error }

journal.cli.importer.text.unconfirmed = text import saved to your journal, but it could not confirm the entries are ready to search; run it again

journal.cli.importer.error.unknown.source = unknown importer source: { $source }

journal.cli.importer.error.apple.inspect = could not inspect Apple Health export; retry with a valid export

journal.cli.importer.arg.usage = usage: solstone journal importer [-h] [options] media [timestamp]

journal.cli.importer.arg.error = solstone journal importer: error: { $detail }

journal.cli.importer.arg.media.required = the following arguments are required: media

journal.cli.importer.arg.expected.one = argument { $argument }: expected one argument

journal.cli.importer.arg.unrecognized = unrecognized arguments: { $arguments }

journal.cli.importer.arg.archive.identity.scope = --archive-identity is only valid with --audio-archive

journal.cli.importer.arg.facet.scope = --facet is only valid when importing one audio or text file or a body import

journal.cli.importer.arg.offset.limit.scope = unrecognized arguments: --offset/--limit are only valid with --audio-archive-members

journal.cli.importer.arg.archive.conflict.commands = unrecognized arguments: conflicting audio archive commands

journal.cli.importer.arg.archive.conflict.flags = unrecognized arguments: conflicting audio archive flags

journal.cli.importer.arg.resume.scope = unrecognized arguments: --resume is only valid with --audio-archive

journal.cli.importer.arg.source.zone.scope = unrecognized arguments: --source-zone is only valid with --audio-archive

journal.cli.importer.arg.invalid.int = argument { $flag }: invalid int value: '{ $value }'

journal.cli.importer.arg.archive.identity.sha256 = --archive-identity must be a SHA-256 identity

journal.cli.importer.audio.archive.failed = audio archive failed: { $error }

journal.cli.importer.audio.archive.zone.changed = the time zone changed after preview; preview it again

journal.cli.importer.audio.archive.folder.needs.zone = a folder requires --source-zone

journal.cli.importer.audio.archive.changed = the archive changed after preview; preview it again

journal.cli.importer.audio.archive.dry.run.resume = use --audio-archive-status to inspect an admitted run

journal.cli.importer.audio.archive.preview.line1 = audio archive preview: { $members }, { $source_bytes }, { $days }, { $tiles }

journal.cli.importer.audio.archive.admitted.line1 = archive { $run_id }: { $members }, { $source_bytes }, { $days }, { $tiles }

journal.cli.importer.audio.archive.span.line = span: { $span }

journal.cli.importer.audio.archive.span.none = none

journal.cli.importer.audio.archive.span.range = { $first } to { $last }

journal.cli.importer.audio.archive.left.out.inline = left out: { $counts }

journal.cli.importer.audio.archive.progress.saved = saved { $saved } of { $total } ({ $tiles })

journal.cli.importer.audio.archive.complete.line1 = audio archive { $operation }: { $run_id }

journal.cli.importer.audio.archive.complete.line2 = saved { $parts } in { $tiles }; { $pending } pending processing, { $processed } processed, { $unavailable } unavailable, { $rejected } rejected

journal.cli.importer.audio.archive.status.failed = audio archive status failed: { $error }

journal.cli.importer.audio.archive.status.summary = audio archive status: run_id={ $run_id } total_parts={ $total_parts } saved_parts={ $saved_parts } saved_tiles={ $saved_tiles } pending_processing={ $pending } processed_parts={ $processed } unavailable_parts={ $unavailable } rejected_parts={ $rejected } save_complete={ $save_complete }

journal.cli.importer.audio.archive.left.out.header = left out:

journal.cli.importer.audio.archive.members.failed = audio archive members failed: { $error }

journal.cli.importer.audio.archive.members.summary = audio archive members: run_id={ $run_id } offset={ $offset } count={ $count }

journal.cli.importer.registry.preview.failed = import preview failed: { $error }

journal.cli.importer.registry.originals.choice.unreadable = couldn't read the keep originals choice. reopen the import to try again.

journal.cli.importer.registry.gemini.folder.unreadable = Gemini import failed. couldn't read the saved export folder.

journal.cli.importer.registry.found.nothing = { $source } import failed: found nothing to import

journal.cli.importer.registry.originals.keep.failed = couldn't keep a copy of the original. try again.

journal.cli.importer.registry.attempt.superseded = { $source } import failed: attempt { $import_id }:{ $generation } was superseded

journal.cli.importer.registry.unconfirmed = { $source } import saved to your journal, but it could not confirm the entries are ready to search; run it again

journal.cli.importer.registry.originals.cleanup.failed = saved to your journal. couldn't remove the extra copy.

journal.cli.importer.registry.evernote.attachments.failed = Evernote import failed. { $error }

journal.cli.importer.registry.summary.pdf = imported { $documents }

journal.cli.importer.registry.summary.image = imported 1 image

journal.cli.importer.registry.archive.missing.provenance = { $source } import failed: missing provenance for attempt { $attempt }

journal.cli.importer.registry.archive.missing.facts = { $source } import failed: missing attempt facts for { $attempt }

journal.cli.importer.registry.archive.record.merge.failed = { $source } import failed: failed to record merge results: { $error }

journal.cli.importer.registry.archive.record.completed.failed = { $source } import failed: failed to record completed attempt: { $error }

journal.cli.importer.registry.archive.record.attempt.failed = { $source } import failed: failed to record attempt: { $error }

journal.cli.importer.registry.archive.error.and.record.failed = { $source } import failed: { $error }; failed to record attempt: { $record_error }

journal.cli.importer.registry.strava.release.failed = workouts placed at the wrong time could not be released

journal.cli.importer.registry.strava.discard.failed = could not remove the staged Strava download: { $error }

journal.cli.importer.registry.strava.filing.failed = { $source } import failed: the workouts couldn't be filed: { $error }

journal.cli.importer.summary.conversations = imported { $messages } from { $conversations } across { $days }

journal.cli.importer.preview.chatgpt = { $messages } from ChatGPT export

journal.cli.importer.preview.claude = { $messages } from Claude chat export

journal.cli.importer.preview.gemini = { $messages } from Gemini export

journal.cli.importer.preview.ics.none = No ICS data found

journal.cli.importer.preview.ics.no.events = No events found in ICS data

journal.cli.importer.preview.ics.counts = { $events }, { $attendees }

journal.cli.importer.summary.ics.base = imported { $events } across { $days }

journal.cli.importer.summary.ics.reason.no.change = { $count } with no change time

journal.cli.importer.summary.ics.reason.unknown.zone = { $count } in an unknown time zone

journal.cli.importer.summary.ics.reason.invalid = { $count } unreadable

journal.cli.importer.summary.ics.skipped = ; { $skipped } not listed by the day they happen ({ $reasons })

journal.cli.importer.summary.ics.truncated.series = { $quantity ->
    [one] ; 1 repeating series listed only for its latest { $limit } dates
   *[other] ; { $count } repeating series listed only for their latest { $limit } dates
}

journal.cli.importer.summary.ics.unexpandable.series = { $quantity ->
    [one] ; 1 repeating series listed only for its first date
   *[other] ; { $count } repeating series listed only for their first date
}

journal.cli.importer.summary.ics.over.limit = ; { $count } not listed by the day they happen, past the { $limit } dates one import lists

journal.cli.importer.preview.document.none = No PDF documents found

journal.cli.importer.preview.document.counts = { $documents }, { $pages }

journal.cli.importer.preview.document.unreadable = { $documents }, { $pages }; { $unreadable } unreadable ({ $failures })

journal.cli.importer.summary.document = imported { $documents } across { $days } into { $segments }

journal.cli.importer.preview.image.one = 1 image ({ $format }, { $width }×{ $height })

journal.cli.importer.summary.image.none = No readable image found

journal.cli.importer.summary.archive = Journal archive: { $days }, { $entities }, { $facets } ({ $warnings })

journal.cli.importer.force.pdf.day.notice = notice: this file's name dates to { $fresh }, and a previous import was recorded on { $stored }. the PDF's own dates may select a different day. choose --timestamp to set its date. existing recordings are not moved.
