journal.mac.admin.terminal.failure.title = das Admin-Terminal konnte nicht geöffnet werden

journal.mac.admin.terminal.menu.item.title = Admin-Terminal öffnen

journal.mac.app.journal = journal

journal.mac.backup.setup.hint = eine Sicherung hält dein Journal sicher. richte sie von deinem Journal aus ein.

journal.mac.connection.invalid.response = die Journal-App konnte nicht lesen, was von deinem Journal zurückkam.

journal.mac.connection.link.failed = die Journal-App konnte dein Journal nicht in deinem Browser öffnen. versuch es noch einmal.

journal.mac.connection.refused = die Journal-App konnte sich nicht mit deinem Journal auf diesem Mac verbinden.

journal.mac.connection.timed.out = die Journal-App hat zu lange auf dein Journal gewartet. versuch es noch einmal.

journal.mac.connection.unavailable = dein Journal ist noch nicht bereit für die Journal-App. versuch es in Kürze noch einmal.

journal.mac.devices.add.device = Gerät koppeln

journal.mac.devices.cancel = abbrechen

journal.mac.devices.close = schließen

journal.mac.devices.copied = kopiert ✓

journal.mac.devices.copy.link = Link kopieren

journal.mac.devices.detail.network = { $network } · { $activity }

journal.mac.devices.detail.network.paired = { $network } · { $activity } · gekoppelt { $date }

journal.mac.devices.detail.paired = { $activity } · gekoppelt { $date }

journal.mac.devices.empty.body = kopple ein Gerät, und es tritt deinem Journal bei.

journal.mac.devices.empty.title = noch keine Geräte

journal.mac.devices.expires.in = läuft ab in { $minutes }:{ $seconds }

journal.mac.devices.link.expired.body = öffne einen neuen Link, um ein Gerät zu koppeln.

journal.mac.devices.link.expired.title = Link abgelaufen

journal.mac.devices.loading = Geräte werden geladen...

journal.mac.devices.model.last.seen = zuletzt gesehen { $time }

journal.mac.devices.model.last.seen.just.now = zuletzt gesehen gerade eben

journal.mac.devices.model.last.seen.unknown = zuletzt gesehen unbekannt

journal.mac.devices.model.never.connected = nie verbunden

journal.mac.devices.model.status = Status { $status }

journal.mac.devices.not.ready.body = beende die Ersteinrichtung deines Journals, dann versuch es noch einmal.

journal.mac.devices.not.ready.title = Geräte sind nicht bereit

journal.mac.devices.not.running.body = starte das Journal, dann versuch es noch einmal.

journal.mac.devices.not.running.title = Journal läuft nicht

journal.mac.devices.open.fresh.link = einen neuen Link öffnen

journal.mac.devices.paired.done = fertig

journal.mac.devices.paired.heading = "{ $label }" kann dein Journal jetzt erreichen

journal.mac.devices.paired.heading.unnamed = ein neues Gerät kann dein Journal jetzt erreichen

journal.mac.devices.paired.note = prüfe auf diesem Gerät, ob es dieselbe mark zeigt, dann antworte dort. hast du es nicht gekoppelt?

journal.mac.devices.paired.note.anywhere = prüfe auf diesem Gerät, ob es dieselbe mark zeigt, dann antworte dort. es kann dein Journal von überall erreichen. hast du es nicht gekoppelt?

journal.mac.devices.paired.remove.failed = dieses Gerät konnte nicht entkoppelt werden. versuch es noch einmal, oder entferne es unter Geräte.

journal.mac.devices.paired.removed = entkoppelt. das Gerät kann dein Journal nicht erreichen, bis es wieder gekoppelt ist.

journal.mac.devices.paired.unpair = dieses Gerät entkoppeln

journal.mac.devices.pairing.code = Kopplungscode

journal.mac.devices.pairing.failed.title = die Kopplung konnte nicht geöffnet werden

journal.mac.devices.pairing.instructions = scanne diesen Code oder öffne den Link auf dem Gerät, das du koppeln willst.

journal.mac.devices.pairing.link.label = Kopplungslink

journal.mac.devices.pairing.opening = Kopplung wird geöffnet...

journal.mac.devices.pairing.title = Gerät koppeln

journal.mac.devices.pane.title = Geräte

journal.mac.devices.peer.journals.header = Peer-Journale

journal.mac.devices.remove = entfernen

journal.mac.devices.revoke.body = { $name } verliert den Zugriff auf dein Journal. du kannst es später wieder koppeln.{ $space1 }{ $detail }

journal.mac.devices.revoke.failed = dieses Gerät konnte nicht entfernt werden

journal.mac.devices.revoke.title = { $name } entfernen?

journal.mac.devices.try.again = erneut versuchen

journal.mac.devices.unnamed.device = unbenanntes Gerät

journal.mac.devices.unnamed.journal = unbenanntes Journal

journal.mac.devices.your.devices.header = deine Geräte

journal.mac.events.completed = Ersteinrichtung ok

journal.mac.events.completed.duration = Ersteinrichtung ok ({ $duration_ms }ms)

journal.mac.events.doctor.check = { $space0 }[{ $label }] { $name } - { $detail }

journal.mac.events.doctor.started = doctor gestartet

journal.mac.events.doctor.status = doctor-Status: { $status }

journal.mac.events.doctor.summary = { $total ->
    [one] doctor: { $total } Prüfung ({ $failed } fehlgeschlagen, { $warnings } Warnungen, { $skipped } übersprungen)
   *[other] doctor: { $total } Prüfungen ({ $failed } fehlgeschlagen, { $warnings } Warnungen, { $skipped } übersprungen)
}

journal.mac.events.done = Schritt { $step } fertig ({ $outcome })

journal.mac.events.done.duration = Schritt { $step } fertig ({ $outcome }, { $duration_ms }ms)

journal.mac.events.fail = fehlgeschlagen

journal.mac.events.failed = Ersteinrichtung fehlgeschlagen

journal.mac.events.failed.step = Ersteinrichtung fehlgeschlagen bei { $failed_step }

journal.mac.events.malformed = [malformed event] fehlerhaftes Ereignis { $raw }

journal.mac.events.ok = ok

journal.mac.events.skip = übersprungen

journal.mac.events.skipped = Schritt { $step } übersprungen

journal.mac.events.skipped.reason = Schritt { $step } übersprungen ({ $reason })

journal.mac.events.started = Ersteinrichtung gestartet

journal.mac.events.started.version = Ersteinrichtung gestartet (Version { $version }, { $mode })

journal.mac.events.step = Schritt: { $step }

journal.mac.events.step.failed = Schritt { $step } fehlgeschlagen: { $message }

journal.mac.events.step.indexed = Schritt { $index }/{ $total }: { $step }

journal.mac.events.unknown = unbekannt

journal.mac.events.unparseable = [unparseable line] nicht auswertbare Zeile { $raw }

journal.mac.events.unrecognized = [unrecognized event] unbekanntes Ereignis { $raw }

journal.mac.events.warn = Warnung

journal.mac.events.warning = { $space0 }Warnung: { $text }

journal.mac.events.warning.fix = { $space0 }Warnung: { $text } (Behebung: { $fix_hint })

journal.mac.first.run.model.setup.did.not.finish = die Ersteinrichtung wurde nicht abgeschlossen

journal.mac.first.run.model.setup.reported.completion.twice = die Ersteinrichtung hat den Abschluss zweimal gemeldet

journal.mac.first.run.model.setup.timed.out = Zeitüberschreitung der Ersteinrichtung

journal.mac.health.check.denied = abgelehnt

journal.mac.health.check.granted = gewährt

journal.mac.health.check.microphone = Mikrofon

journal.mac.health.check.screen.recording = Bildschirmaufnahme

journal.mac.health.check.unknown.checking.permissions = unbekannt, Berechtigungen werden geprüft

journal.mac.health.microphone.permission.help = öffne die Systemeinstellungen › Datenschutz & Sicherheit und erlaube das Mikrofon für { $product }

journal.mac.health.screen.permission.help = öffne die Systemeinstellungen › Datenschutz & Sicherheit und erlaube Bildschirm- und Systemaudioaufnahme für { $product }

journal.mac.home.browser.explanation = diese App hält dein Journal auf diesem Mac am Laufen. dein Journal selbst öffnet sich in deinem Browser.

journal.mac.home.unconfigured = hier ist noch nichts. als Nächstes wird dein Journal erstellt.

journal.mac.info.cfbundle.display.name = journal

journal.mac.info.cfbundle.name = journal

journal.mac.info.nshuman.readable.copyright = © 2026 { $company }. alle Rechte vorbehalten.

journal.mac.local.endpoint.unavailable = dein Journal ist noch nicht bereit für die Journal-App. versuch es in Kürze noch einmal.

journal.mac.location.cancel = abbrechen

journal.mac.location.leave.setup.confirm = die Ersteinrichtung stoppt.

journal.mac.location.new.journal.hosted.share = wenn du die Sicherung dieses Journals dort wiederherstellst, übernimmt das neue Journal sie, und dieses sichert nicht mehr, bis du sie wieder einschaltest.

journal.mac.location.other.journals = andere Journale auf diesem Mac

journal.mac.location.same.mark.template = beide haben dieselbe mark, deshalb erreichen deine Geräte das Journal, das gerade läuft.

journal.mac.location.shared.hosted.template = das Journal in { $path } teilt sich diese von { $company } betriebene verschlüsselte Sicherung, und nur eines kann sie nutzen. wenn dieses Journal nicht mehr sichert, schalte sie unter Sicherung wieder ein.

journal.mac.location.start.new.journal = ein neues Journal starten

journal.mac.location.start.new.journal.confirm.template = dein Journal in { $path } stoppt und bleibt dort, unverändert. als Nächstes beginnt die Ersteinrichtung, in der du aus einer Sicherung wiederherstellen kannst.

journal.mac.location.start.new.journal.confirm.title = ein neues Journal starten?

journal.mac.location.start.new.journal.help = dein Journal stoppt und bleibt in seinem Ordner, unverändert. während der Ersteinrichtung kannst du aus einer Sicherung wiederherstellen, und du kannst hier zu diesem zurückkehren.

journal.mac.location.switch.button = wechseln

journal.mac.location.switch.confirm.template = dein Journal in { $path } stoppt und bleibt dort, unverändert. du kannst hier zu ihm zurückkehren.

journal.mac.location.switch.confirm.title.template = zum Journal in { $path } wechseln?

journal.mac.location.switch.to.journal = zu diesem Journal wechseln

journal.mac.restart.journal.path.unreadable = Neustart fehlgeschlagen: der Pfad des Journals konnte nicht gelesen werden

journal.mac.restart.restart.failed.journal.did.not.come.back = Neustart fehlgeschlagen: das Journal kam nicht zurück

journal.mac.restart.restart.failed.journal.did.not.restart = Neustart fehlgeschlagen: das Journal wurde nicht neu gestartet

journal.mac.runtime.bundle.missing = die mitgelieferte native Laufzeitumgebung des Journals fehlt unter { $path }

journal.mac.runtime.child.admission.failed = die Zulassung des Journal-Kindprozesses konnte nicht überprüft werden

journal.mac.runtime.child.containment.pending = die Eindämmung des Journal-Kindprozesses läuft

journal.mac.runtime.child.unconfigured = der Journal-Kindprozess ist nicht konfiguriert

journal.mac.runtime.couldn.t.get.ready.continuing = { $library } konnte nicht bereit gemacht werden; es geht weiter

journal.mac.runtime.executable.missing = die mitgelieferte native Programmdatei von { $product } fehlt unter { $path }

journal.mac.runtime.installer.readiness.gate.failed = das Journal konnte für diesen Mac nicht bereit gemacht werden

journal.mac.runtime.journal.child.breaker.tripped = dein Journal hat unerwartet mehrmals innerhalb einer Minute gestoppt

journal.mac.runtime.journal.child.containment.unresolved = dein Journal hat gestoppt, und nicht alle seine Prozesse konnten als geschlossen bestätigt werden

journal.mac.runtime.journal.drive.cannot.run = dein Journal kann auf diesem Laufwerk nicht laufen, weil das Laufwerk Dateien nicht so speichern kann, wie dein Journal es braucht. verwende ein Mac-formatiertes Laufwerk für dein Journal.

journal.mac.runtime.journal.materialize.failed = die Laufzeitumgebung des Journals konnte nicht vorbereitet werden

journal.mac.runtime.journal.readiness.timeout = das Journal wurde nicht rechtzeitig bereit

journal.mac.runtime.journal.setup.needed.before.upgrade = Ersteinrichtung des Journals nötig, bevor das Upgrade weitergehen kann

journal.mac.runtime.journal.spawn.blocked.legacy.service = etwas anderes ist eingerichtet, um dein Journal auszuführen

journal.mac.runtime.journal.spawn.blocked.ports = die Ports des Journals sind noch belegt

journal.mac.runtime.journal.spawn.port.check.failed = es konnte nicht überprüft werden, ob die Ports des Journals frei sind

journal.mac.runtime.journal.spawn.port.held.elsewhere = dein Journal kann nicht starten, weil eine andere App auf diesem Mac die Verbindung nutzt, die es braucht. auf einem gemeinsam genutzten Mac ist das meist das Journal eines anderen Mac-Kontos.

journal.mac.runtime.journal.spawn.service.check.failed = es ließ sich nicht feststellen, ob etwas anderes dein Journal ausführt

journal.mac.runtime.release.identity.missing = journal.app fehlt eine gültige Release-Identität

journal.mac.runtime.required.models.failed = { $command } fehlgeschlagen (Exit { $exit_code }, Beendigung { $reason })

journal.mac.settings.backup = Sicherung

journal.mac.settings.copied = kopiert

journal.mac.settings.copy = kopieren

journal.mac.settings.couldn.t.copy.select.the.text.and.copy.it = das Kopieren ist nicht gelungen. wähle den Text aus und kopiere ihn.

journal.mac.settings.disk.used = belegter Speicherplatz

journal.mac.settings.health = Zustand

journal.mac.settings.launch.the.journal.when.you.log.in = das Journal starten, wenn du dich anmeldest

journal.mac.settings.location = Ort

journal.mac.settings.open.backup = Sicherung öffnen

journal.mac.settings.open.your.journal = dein Journal öffnen

journal.mac.settings.restart = neu starten

journal.mac.settings.run.state = Laufzustand

journal.mac.settings.run.state.link = Laufzustand →

journal.mac.settings.start = starten

journal.mac.settings.startup = Autostart

journal.mac.settings.stop = stoppen

journal.mac.settings.the.journal.app.is.available = die Journal-App { $version } ist verfügbar.

journal.mac.settings.the.journal.update.check.failed = die Suche nach Aktualisierungen des Journals ist fehlgeschlagen.

journal.mac.settings.update.failed.with.version = die Suche nach Aktualisierungen ist fehlgeschlagen. die Journal-App { $version } wurde zuvor gefunden.

journal.mac.settings.update.needs.attention = Aktualisierung braucht Aufmerksamkeit

journal.mac.settings.view.journal.updates = Journal-Aktualisierungen anzeigen

journal.mac.settings.your.journal.at.a.glance = dein Journal, auf einen Blick

journal.mac.settings.your.journal.mark.not.read.yet = dein Journal, mark noch nicht gelesen

journal.mac.setup.adopt.failed = dieses Journal konnte nicht geöffnet werden

journal.mac.setup.adopt.landing.line = nichts wurde verschoben. dein Journal war immer hier. du erkennst es an seiner mark.

journal.mac.setup.adopt.opening = dieses Journal wird geöffnet

journal.mac.setup.adopt.title = Journal gefunden

journal.mac.setup.back = zurück

journal.mac.setup.choose.location = wählen

journal.mac.setup.chrome.of = { $done } von { $total }

journal.mac.setup.continue.button = fortfahren

journal.mac.setup.continue.in.the.solstone.app = in der { $product }-App fortfahren

journal.mac.setup.download.bytes = { $done } / { $total }

journal.mac.setup.finish.in.browser.line = dieses Fenster geht weiter, wenn du fertig bist.

journal.mac.setup.finish.in.browser.title = Ersteinrichtung in deinem Browser abschließen

journal.mac.setup.folder.holds.another.journal = dieser Ordner überschneidet sich mit einem anderen Journal auf diesem Mac. wähle einen anderen Ordner.

journal.mac.setup.go.back.to.the.journal.in = zum Journal in { $path } zurückgehen

journal.mac.setup.handoff.competing.setup = die { $product }-App schließt schon eine andere Ersteinrichtung ab. schließ sie dort ab, oder öffne dieses Journal hier.

journal.mac.setup.handoff.held.pairing = die { $product }-App ist schon mit einem Journal verbunden. öffne die { $product }-App, um diese Verbindung zu ändern, oder öffne dieses Journal hier.

journal.mac.setup.handoff.not.delivered = in der { $product }-App konnte nicht mit der Ersteinrichtung fortgefahren werden.

journal.mac.setup.handoff.pending = die Ersteinrichtung ist in der { $product }-App nicht weitergegangen. öffne die App, um fortzufahren, oder versuch es hier noch einmal.

journal.mac.setup.journal.folder = Journalordner

journal.mac.setup.location.field = Ort

journal.mac.setup.login.item.change.failed = der Start bei der Anmeldung konnte nicht geändert werden.

journal.mac.setup.login.item.unavailable = der Start bei der Anmeldung ist von diesem Ort der App aus nicht verfügbar.

journal.mac.setup.name.location.title = lass uns dein Journal auf diesem Mac erstellen

journal.mac.setup.on.this.mac = auf diesem Mac

journal.mac.setup.on.this.mac.description = Anfragen an das Modell bleiben auf diesem Mac. fortfahren startet das Herunterladen.

journal.mac.setup.open.your.journal = dein Journal öffnen

journal.mac.setup.reopen.in.browser.button = Ersteinrichtungsseite öffnen

journal.mac.setup.restore.from.backup = aus einer Sicherung wiederherstellen

journal.mac.setup.runner.solstone.journal.setup.failed = { $setup_command } fehlgeschlagen

journal.mac.setup.set.up.solstone = { $product } einrichten

journal.mac.setup.setup.progress.not.saved = der Fortschritt der Ersteinrichtung konnte nicht gespeichert werden.

journal.mac.setup.setup.subtitle = das kann eine Minute dauern.

journal.mac.setup.setup.title = dein Journal wird eingerichtet

journal.mac.setup.start.automatically.help = automatisch starten, nachdem du dich auf diesem Mac angemeldet hast.

journal.mac.setup.start.setup = Ersteinrichtung starten

journal.mac.setup.step.progress = Schritt { $step } von 7

journal.mac.setup.the.solstone.app = die { $product }-App

journal.mac.setup.the.solstone.app.description = läuft auf diesem Mac. dein Mikrofon, dein Bildschirm und der Ton deines Computers bleiben jeweils aus, bis du sie einschaltest.

journal.mac.setup.thinking = Denken

journal.mac.setup.thinking.description = hilft deinem Journal zu verstehen, was es behält. du wählst während der Ersteinrichtung oder später, wo.

journal.mac.setup.thinking.failed = dieser Schritt konnte nicht abgeschlossen werden. versuch es noch einmal.

journal.mac.setup.try.again = erneut versuchen

journal.mac.setup.what.this.sets.up = was hier eingerichtet wird

journal.mac.setup.what.this.sets.up.journal = lebt auf diesem Mac und behält alles, was hineingeht.

journal.mac.setup.where.does.thinking.happen = wo findet das Denken statt?

journal.mac.setup.where.does.thinking.happen.description = dein Journal bleibt auf diesem Mac, egal, was du wählst. das entscheidet nur, wo das Denken stattfindet.

journal.mac.setup.where.your.journal.lives = wo dein Journal liegt

journal.mac.setup.where.your.journal.lives.description = alles, was dein Journal behält, bleibt in diesem Ordner, auf diesem Mac.

journal.mac.setup.your.journal = dein Journal

journal.mac.updates.deferred = die Aktualisierung deines Journals geht weiter, nachdem dein Journal bereit ist.

journal.mac.updates.ready.to.install = die Journal-App { $version } ist heruntergeladen und bereit zum Installieren.

journal.mac.window.model.backup = Sicherung

journal.mac.window.model.blocked = blockiert

journal.mac.window.model.devices = Geräte

journal.mac.window.model.healthy = in Ordnung

journal.mac.window.model.home = Start

journal.mac.window.model.journal = journal

journal.mac.window.model.location = Ort

journal.mac.window.model.not.set = nicht festgelegt

journal.mac.window.model.run.state = Laufzustand

journal.mac.window.model.running = läuft

journal.mac.window.model.starting = wird gestartet…

journal.mac.window.model.startup = Autostart

journal.mac.window.model.stopped = gestoppt

journal.mac.window.model.unknown = unbekannt

journal.mac.window.model.updates = Aktualisierungen
