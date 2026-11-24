journal.window.backup.age.backup.failed.clock = Sicherungsversuch ist nicht gelungen · { $recorded_at }

journal.window.backup.age.backup.failed.day = { $quantity ->
        [one] Sicherungsversuch ist vor { $count } Tag nicht gelungen
       *[other] Sicherungsversuch ist vor { $count } Tagen nicht gelungen
    }

journal.window.backup.age.backup.failed.hour = { $quantity ->
        [one] Sicherungsversuch ist vor { $count } Stunde nicht gelungen
       *[other] Sicherungsversuch ist vor { $count } Stunden nicht gelungen
    }

journal.window.backup.age.backup.failed.minute = { $quantity ->
        [one] Sicherungsversuch ist vor { $count } Minute nicht gelungen
       *[other] Sicherungsversuch ist vor { $count } Minuten nicht gelungen
    }

journal.window.backup.age.backup.failed.month = { $quantity ->
        [one] Sicherungsversuch ist vor { $count } Monat nicht gelungen
       *[other] Sicherungsversuch ist vor { $count } Monaten nicht gelungen
    }

journal.window.backup.age.backup.failed.second = { $quantity ->
        [one] Sicherungsversuch ist vor { $count } Sekunde nicht gelungen
       *[other] Sicherungsversuch ist vor { $count } Sekunden nicht gelungen
    }

journal.window.backup.age.backup.failed.week = { $quantity ->
        [one] Sicherungsversuch ist vor { $count } Woche nicht gelungen
       *[other] Sicherungsversuch ist vor { $count } Wochen nicht gelungen
    }

journal.window.backup.age.check.failed.clock = Stichprobe ist nicht gelungen · { $recorded_at }

journal.window.backup.age.check.failed.day = { $quantity ->
        [one] Stichprobe ist vor { $count } Tag nicht gelungen
       *[other] Stichprobe ist vor { $count } Tagen nicht gelungen
    }

journal.window.backup.age.check.failed.hour = { $quantity ->
        [one] Stichprobe ist vor { $count } Stunde nicht gelungen
       *[other] Stichprobe ist vor { $count } Stunden nicht gelungen
    }

journal.window.backup.age.check.failed.minute = { $quantity ->
        [one] Stichprobe ist vor { $count } Minute nicht gelungen
       *[other] Stichprobe ist vor { $count } Minuten nicht gelungen
    }

journal.window.backup.age.check.failed.month = { $quantity ->
        [one] Stichprobe ist vor { $count } Monat nicht gelungen
       *[other] Stichprobe ist vor { $count } Monaten nicht gelungen
    }

journal.window.backup.age.check.failed.second = { $quantity ->
        [one] Stichprobe ist vor { $count } Sekunde nicht gelungen
       *[other] Stichprobe ist vor { $count } Sekunden nicht gelungen
    }

journal.window.backup.age.check.failed.week = { $quantity ->
        [one] Stichprobe ist vor { $count } Woche nicht gelungen
       *[other] Stichprobe ist vor { $count } Wochen nicht gelungen
    }

journal.window.backup.age.check.skipped.clock = Stichprobe ließ sich nicht ausführen · { $recorded_at }

journal.window.backup.age.check.skipped.day = { $quantity ->
        [one] Stichprobe ließ sich vor { $count } Tag nicht ausführen
       *[other] Stichprobe ließ sich vor { $count } Tagen nicht ausführen
    }

journal.window.backup.age.check.skipped.hour = { $quantity ->
        [one] Stichprobe ließ sich vor { $count } Stunde nicht ausführen
       *[other] Stichprobe ließ sich vor { $count } Stunden nicht ausführen
    }

journal.window.backup.age.check.skipped.minute = { $quantity ->
        [one] Stichprobe ließ sich vor { $count } Minute nicht ausführen
       *[other] Stichprobe ließ sich vor { $count } Minuten nicht ausführen
    }

journal.window.backup.age.check.skipped.month = { $quantity ->
        [one] Stichprobe ließ sich vor { $count } Monat nicht ausführen
       *[other] Stichprobe ließ sich vor { $count } Monaten nicht ausführen
    }

journal.window.backup.age.check.skipped.second = { $quantity ->
        [one] Stichprobe ließ sich vor { $count } Sekunde nicht ausführen
       *[other] Stichprobe ließ sich vor { $count } Sekunden nicht ausführen
    }

journal.window.backup.age.check.skipped.week = { $quantity ->
        [one] Stichprobe ließ sich vor { $count } Woche nicht ausführen
       *[other] Stichprobe ließ sich vor { $count } Wochen nicht ausführen
    }

journal.window.backup.age.elapsed.clock = { $recorded_at }

journal.window.backup.age.elapsed.day = { $quantity ->
        [one] vor { $count } Tag
       *[other] vor { $count } Tagen
    }

journal.window.backup.age.elapsed.hour = { $quantity ->
        [one] vor { $count } Stunde
       *[other] vor { $count } Stunden
    }

journal.window.backup.age.elapsed.minute = { $quantity ->
        [one] vor { $count } Minute
       *[other] vor { $count } Minuten
    }

journal.window.backup.age.elapsed.month = { $quantity ->
        [one] vor { $count } Monat
       *[other] vor { $count } Monaten
    }

journal.window.backup.age.elapsed.second = { $quantity ->
        [one] vor { $count } Sekunde
       *[other] vor { $count } Sekunden
    }

journal.window.backup.age.elapsed.week = { $quantity ->
        [one] vor { $count } Woche
       *[other] vor { $count } Wochen
    }

journal.window.backup.capacity.gib = { $amount } { $unit }

journal.window.backup.capacity.mib = { $amount } { $unit }

journal.window.backup.consent.destination.backup.only.days = { $days_quantity ->
        [one] 1 Tag deines Journals ({ $backup_size }) liegt nur in jener Sicherung.
       *[other] { $days } Tage deines Journals ({ $backup_size }) liegen nur in jener Sicherung.
    }

journal.window.backup.consent.teardown.backup.only.days = { $days_quantity ->
        [one] { $days } Tag deines Journals ({ $backup_size }) liegt nur in dieser Sicherung. das Löschen der Sicherung löscht ihn überall, für immer.
       *[other] { $days } Tage deines Journals ({ $backup_size }) liegen nur in dieser Sicherung. das Löschen der Sicherung löscht sie überall, für immer.
    }

journal.window.backup.copy.action.labels.backup.now = jetzt eine Sicherung anlegen

journal.window.backup.copy.action.labels.cancel = abbrechen

journal.window.backup.copy.action.labels.change.destination = Ziel ändern

journal.window.backup.copy.action.labels.check.cleanup = erneut prüfen

journal.window.backup.copy.action.labels.cleanup.blocked = es ließ sich nicht bestätigen, dass eine Journal-Aufgabe gestoppt ist. sie muss stoppen, bevor Sicherungsaktionen weitergehen können.

journal.window.backup.copy.action.labels.cleanup.checking = Verfügbarkeit der Sicherung wird geprüft

journal.window.backup.copy.action.labels.cleanup.contended = versuch es gleich noch einmal zu prüfen.

journal.window.backup.copy.action.labels.cleanup.waiting = Sicherungsaktionen warten

journal.window.backup.copy.action.labels.enable = Sicherung einschalten

journal.window.backup.copy.action.labels.finish.cleanup = Stoppen der Sicherung abschließen

journal.window.backup.copy.action.labels.finish.task = Stoppen der Aufgabe abschließen

journal.window.backup.copy.action.labels.finishing.cleanup = Sicherung wird gestoppt…

journal.window.backup.copy.action.labels.finishing.task = Aufgabe wird gestoppt…

journal.window.backup.copy.action.labels.pause = Sicherung pausieren

journal.window.backup.copy.action.labels.restore = wiederherstellen

journal.window.backup.copy.action.labels.resume = Sicherung fortsetzen

journal.window.backup.copy.action.labels.rotate.key = Wiederherstellungsschlüssel neu erzeugen

journal.window.backup.copy.action.labels.save.destination = Ziel speichern

journal.window.backup.copy.action.labels.save.retention = Aufbewahrung speichern

journal.window.backup.copy.action.labels.start = loslegen

journal.window.backup.copy.action.labels.teardown = Sicherung ausschalten und löschen

journal.window.backup.copy.action.labels.try.again = erneut versuchen

journal.window.backup.copy.action.labels.understand = ich verstehe

journal.window.backup.copy.action.labels.view.key = Wiederherstellungsschlüssel anzeigen

journal.window.backup.copy.brand.lock = dein Journal ist immer privat, nur deins.

journal.window.backup.copy.confirm.escape = Schlüssel erneut anzeigen

journal.window.backup.copy.confirm.prompt = gib den Wiederherstellungsschlüssel ein, den du gerade notiert hast.

journal.window.backup.copy.destination.edit.checking = Ziel wird geprüft…

journal.window.backup.copy.destination.edit.confirm.action = neues Ziel verwenden

journal.window.backup.copy.destination.edit.confirm.days.unavailable = gerade lässt sich nicht prüfen, wie viele Tage deines Journals nur in jener Sicherung liegen.

journal.window.backup.copy.destination.edit.confirm.lead = deine bestehende Sicherung bleibt, wo sie ist, unberührt. nach dem Wechsel legt dein Journal dort keine mehr an und stellt von dort nicht wieder her.

journal.window.backup.copy.destination.edit.confirm.title = woanders eine Sicherung anlegen?

journal.window.backup.copy.destination.edit.keep.hint = lass beide leer, um die Zugangsdaten zu behalten, die du jetzt verwendest.

journal.window.backup.copy.destination.edit.lead = ändere das Repository, die Zugangsdaten oder beides. dein Wiederherstellungsschlüssel bleibt gleich.

journal.window.backup.copy.destination.edit.no.change = das ist schon dein Ziel.

journal.window.backup.copy.destination.edit.partial.credentials = gib beide ein, oder lass beide leer, um die Zugangsdaten zu behalten, die du jetzt verwendest.

journal.window.backup.copy.destination.edit.refusals.auth.failed = das Ziel hat die Zugangsdaten abgelehnt, also wurde nichts gespeichert. prüfe sie und versuch es noch einmal.

journal.window.backup.copy.destination.edit.refusals.bucket.not.found = diese Zugangsdaten können keinen Bucket mit diesem Namen öffnen, also wurde nichts gespeichert. prüfe den Bucket-Namen im Repository.

journal.window.backup.copy.destination.edit.refusals.destination.unchanged = das ist schon dein Ziel.

journal.window.backup.copy.destination.edit.refusals.locked = die Sicherung dort ist beschäftigt und nichts wurde gespeichert. versuch es gleich noch einmal.

journal.window.backup.copy.destination.edit.refusals.repository.key.refused = dort liegt schon eine Sicherung, die dein Wiederherstellungsschlüssel nicht öffnen kann, also wurde nichts gespeichert. wähle ein anderes Ziel.

journal.window.backup.copy.destination.edit.refusals.timeout = das Ziel hat nicht rechtzeitig geantwortet, also wurde nichts gespeichert. prüfe Repository und Zugangsdaten, dann versuch es noch einmal.

journal.window.backup.copy.destination.edit.refusals.unreachable = das Ziel ließ sich nicht öffnen, also wurde nichts gespeichert. prüfe das Repository und die Zugangsdaten, dann versuch es noch einmal.

journal.window.backup.copy.destination.edit.saved.credentials = gespeichert. die neuen Zugangsdaten öffnen deine Sicherung.

journal.window.backup.copy.destination.edit.saved.existing = gespeichert. dort liegt schon eine Sicherung, die dein Journal öffnen kann, also verwendet es diese.

journal.window.backup.copy.destination.edit.saved.new = gespeichert. dort war noch keine Sicherung, also hat dein Journal eine neue eingerichtet.

journal.window.backup.copy.destination.field.labels.access.key.id = Zugriffsschlüssel-ID

journal.window.backup.copy.destination.field.labels.b2.application.key = Anwendungsschlüssel

journal.window.backup.copy.destination.field.labels.b2.key.id = Schlüssel-ID

journal.window.backup.copy.destination.field.labels.backend = backend

journal.window.backup.copy.destination.field.labels.repository = Repository

journal.window.backup.copy.destination.field.labels.secret.access.key = geheimer Zugriffsschlüssel

journal.window.backup.copy.destination.modes.byo.desc = dein Bucket, deine Zugangsdaten. der Standard.

journal.window.backup.copy.destination.modes.byo.note = { $slot1 } ist nie im Pfad.

journal.window.backup.copy.destination.modes.byo.title = dein eigenes

journal.window.backup.copy.destination.modes.hosted.cta = Sicherung einrichten →

journal.window.backup.copy.destination.modes.hosted.desc = { $slot1 } betreibt den Teil außerhalb des Geräts für dich.

journal.window.backup.copy.destination.modes.hosted.note = { $slot1 } hält immer nur eine verschlüsselte Kopie, die auf deinem Gerät verschlüsselt wurde, bevor sie das Gerät verlässt.

journal.window.backup.copy.destination.modes.hosted.title = von { $slot1 } betrieben

journal.window.backup.copy.destination.object.lock.summary = Hinweise zur Bucket-Einrichtung

journal.window.backup.copy.destination.object.lock.warning = aktiviere Compliance-mode Object Lock am Bucket nicht. es widerspricht dem Ausdünnen der Sicherung und dem Lock-Aufräumen. brauchst du Unveränderlichkeit, verwend Governance mode.

journal.window.backup.copy.destination.reason.labels.auth.failed = das Ziel hat den Schlüssel oder die Zugangsdaten abgelehnt. prüfe den Wiederherstellungsschlüssel und die Angaben zum Ziel.

journal.window.backup.copy.destination.reason.labels.bucket.not.found = diese Zugangsdaten können keinen Bucket dieses Namens öffnen. prüfe, dass das Repository den Bucket-Namen nutzt, nicht den deines Speicherschlüssels. fehlt der Bucket noch, erstelle ihn zuerst bei deinem Speicheranbieter.

journal.window.backup.copy.destination.reason.labels.locked = das Ziel ist beschäftigt. versuch es gleich noch einmal.

journal.window.backup.copy.destination.reason.labels.repo.exists = Ziel ist erreichbar und schon eingerichtet.

journal.window.backup.copy.destination.reason.labels.repo.missing = Ziel ist erreichbar und muss eingerichtet werden.

journal.window.backup.copy.destination.reason.labels.timeout = das Ziel hat zu lange gebraucht, um zu antworten. versuch es gleich noch einmal.

journal.window.backup.copy.destination.reason.labels.unreachable = das Ziel war nicht erreichbar. prüfe den Ort der Sicherung und versuch es noch einmal.

journal.window.backup.copy.destination.repository.hint = das restic-Repository für deinen Bucket, z. B. { $slot1 }

journal.window.backup.copy.educate.stakes = Wiederherstellen braucht zweierlei: deinen Wiederherstellungsschlüssel und entweder deine Speicherangaben oder eine Anmeldung am Diensteportal. verlierst du ihn, kann niemand die Sicherung wiederherstellen, auch { $slot1 } nicht.

journal.window.backup.copy.error.intro = fang beim Wiederherstellungsschlüssel an. wenn es dann immer noch nicht gelingt, prüfe die Angaben zum Ziel.

journal.window.backup.copy.hosted.location.detail.summary = exakter Speicherort

journal.window.backup.copy.hosted.location.label = Speicher, den { $slot1 } betreibt

journal.window.backup.copy.hosted.manage.label = deine Sicherung im Diensteportal verwalten →

journal.window.backup.copy.hosted.setup.hint = das richtet eine verschlüsselte Sicherung ein, betrieben von { $slot1 }. schalte sie im Diensteportal, das aufgeht, fertig ein und komm zurück. dein Journal bleibt auf deinem Gerät; nur die verschlüsselte Kopie geht zum Speicher, den { $slot2 } betreibt, und { $slot3 } kann sie nie lesen.

journal.window.backup.copy.intro.bullets.item1 = auf deinem Gerät verschlüsselt, bevor es das Gerät verlässt

journal.window.backup.copy.intro.bullets.item2 = optional, immer

journal.window.backup.copy.intro.bullets.item3 = jederzeit löschen

journal.window.backup.copy.intro.optional = dein Journal lebt auf deinem Gerät; die Sicherung ist optional.

journal.window.backup.copy.intro.steps = du speicherst einen Wiederherstellungsschlüssel und wählst, wo die verschlüsselte Sicherung liegt. Wiederherstellen braucht ihn und noch eines: deine Speicherangaben oder eine Anmeldung am Diensteportal, wenn { $slot1 } deinen Speicher betreibt.

journal.window.backup.copy.intro.subtitle = mach eine verschlüsselte Kopie deines Journals an einem sicheren Ort. nur du kannst sie lesen.

journal.window.backup.copy.intro.title = verschlüsselte Sicherung

journal.window.backup.copy.key.clipboard.caveat = das Kopieren legt deinen Wiederherstellungsschlüssel in die Zwischenablage. leere sie nach dem Speichern.

journal.window.backup.copy.key.continue = fortfahren

journal.window.backup.copy.key.copy.failed = dein Wiederherstellungsschlüssel ließ sich nicht kopieren. speichere ihn in einer Datei oder schreib ihn auf, bevor du fortfährst.

journal.window.backup.copy.key.copy.label = kopieren

journal.window.backup.copy.key.pm.caution = speichere deinen Wiederherstellungsschlüssel nur in einem Passwortmanager, dem du vertraust. { $slot1 } empfiehlt keinen bestimmten.

journal.window.backup.copy.key.save.failed = dein Wiederherstellungsschlüssel ließ sich nicht speichern. kopiere oder notiere ihn vor dem Fortfahren.

journal.window.backup.copy.key.save.key = Wiederherstellungsschlüssel speichern

journal.window.backup.copy.key.theft.honesty = wer deinen Wiederherstellungsschlüssel hat, kann alles in deiner Sicherung lesen. bewahre ihn auf wie ein Hauptpasswort.

journal.window.backup.copy.management.destructive.action = Sicherung ausschalten und löschen

journal.window.backup.copy.management.destructive.caption = das löscht deine ganze Sicherung. es werden keine neuen Sicherungen angelegt.

journal.window.backup.copy.management.pause.caption = stoppt Sicherungen, Stichproben, Aufräumen und Medienauslagerung, bis du fortsetzt. deine Sicherung, Schlüssel und Verlauf bleiben, wie sie sind.

journal.window.backup.copy.management.pause.title = Sicherung pausieren

journal.window.backup.copy.management.paused.body = keine Sicherungen laufen, bis du fortsetzt. deine Sicherung bleibt, wie sie war, und Wiederherstellen geht noch.

journal.window.backup.copy.management.paused.hosted.note = Pausieren schaltet die verschlüsselte Sicherung im Diensteportal nicht aus.

journal.window.backup.copy.management.paused.title = Sicherung ist pausiert

journal.window.backup.copy.management.retention.hint = wie viele der jüngsten Kopien je Intervall behalten werden.

journal.window.backup.copy.management.retention.labels.daily = täglich

journal.window.backup.copy.management.retention.labels.hourly = stündlich

journal.window.backup.copy.management.retention.labels.monthly = monatlich

journal.window.backup.copy.management.retention.labels.weekly = wöchentlich

journal.window.backup.copy.management.rotate.key.caption = das ersetzt deinen Wiederherstellungsschlüssel. der alte funktioniert nicht mehr, also wird jede gespeicherte Kopie davon nutzlos. schreib den neuen auf, bevor du diese Seite verlässt.

journal.window.backup.copy.management.rotate.key.summary = Wiederherstellungsschlüssel ersetzen

journal.window.backup.copy.management.status.labels.backup.attempt.unknown = der jüngste Sicherungsversuch ist nicht bekannt.

journal.window.backup.copy.management.status.labels.backup.history.note = das ist ein früheres Ergebnis. es bestätigt nicht, was an deinem aktuellen Sicherungsziel gespeichert ist oder ob es überprüft wurde.

journal.window.backup.copy.management.status.labels.backup.history.unknown = die letzte erfolgreiche Sicherung ist nicht bekannt.

journal.window.backup.copy.management.status.labels.destination = wo deine Sicherung liegt

journal.window.backup.copy.management.status.labels.disabled = aus

journal.window.backup.copy.management.status.labels.enabled = an

journal.window.backup.copy.management.status.labels.last.backup = jüngster Sicherungsversuch

journal.window.backup.copy.management.status.labels.last.prune = letztes Aufräumen

journal.window.backup.copy.management.status.labels.last.successful.backup = letzte erfolgreiche Sicherung

journal.window.backup.copy.management.status.labels.last.verification = letzte Überprüfung

journal.window.backup.copy.management.status.labels.not.available = noch nicht verfügbar

journal.window.backup.copy.management.status.labels.not.yet = noch nicht

journal.window.backup.copy.management.status.labels.retention = Aufbewahrung

journal.window.backup.copy.management.status.labels.setup = Wiederherstellungsschlüssel einrichten

journal.window.backup.copy.management.teardown.confirm.prompt = tippe delete zur Bestätigung

journal.window.backup.copy.management.teardown.gate.unavailable.lead = gerade lässt sich nicht überprüfen, was nur in dieser Sicherung liegt. das Löschen der Sicherung kann Tage deines Journals zerstören, die es sonst nirgends gibt.

journal.window.backup.copy.management.teardown.gate.zero.lead = gerade liegt nichts nur in dieser Sicherung. jeder Tag ist noch auf deinem Gerät.

journal.window.backup.copy.management.teardown.restore.first.action = zuerst alles wiederherstellen

journal.window.backup.copy.offload.action.error = Medienauslagerung nicht gelungen. prüfe die Sicherungseinrichtung, dann versuch es noch einmal.

journal.window.backup.copy.offload.actions.check.preparation = Vorbereitung prüfen

journal.window.backup.copy.offload.actions.disable = Medienauslagerung ausschalten

journal.window.backup.copy.offload.actions.enable = Medienauslagerung einschalten

journal.window.backup.copy.offload.actions.prepare = Kopien vorbereiten

journal.window.backup.copy.offload.actions.restore.day = diesen Tag wiederherstellen

journal.window.backup.copy.offload.actions.review.originals = Originale prüfen

journal.window.backup.copy.offload.actions.save = Grenzwerte speichern

journal.window.backup.copy.offload.backup.only.label = nur in der Sicherung behalten

journal.window.backup.copy.offload.disable.note = das stoppt. Tage, die schon in deiner Sicherung sind, bleiben dort, geschützt und wiederherstellbar.

journal.window.backup.copy.offload.enable.hint = wähle, wie viel ältere Medien diesen Computer nach der Überprüfung der Sicherung verlassen können.

journal.window.backup.copy.offload.invalid.limits = gib für jeden Grenzwert eine positive Zahl ein, dann speichere erneut.

journal.window.backup.copy.offload.labels.budget.gb = Budget für Originalmedien

journal.window.backup.copy.offload.labels.budget.short = Budget

journal.window.backup.copy.offload.labels.days = ausgelagerte Tage

journal.window.backup.copy.offload.labels.device.free = freier Speicherplatz

journal.window.backup.copy.offload.labels.device.total = gesamter Speicherplatz

journal.window.backup.copy.offload.labels.eligible = bereit, diesen Computer zu verlassen

journal.window.backup.copy.offload.labels.floor.gb = Untergrenze für freien Speicherplatz

journal.window.backup.copy.offload.labels.floor.short = Untergrenze

journal.window.backup.copy.offload.labels.known = bisher gezählt

journal.window.backup.copy.offload.labels.last.completed = zuletzt abgeschlossen

journal.window.backup.copy.offload.labels.last.offload = letzte Auslagerung

journal.window.backup.copy.offload.labels.last.restore = letzte Wiederherstellung

journal.window.backup.copy.offload.labels.last.verify = letzte Überprüfung

journal.window.backup.copy.offload.labels.projected = auf diesem Computer nach dem Entfernen

journal.window.backup.copy.offload.labels.proposed = vorgeschlagen, diesen Computer zu verlassen

journal.window.backup.copy.offload.labels.raw.media = auf diesem Computer

journal.window.backup.copy.offload.labels.under.one.mib = unter 1 { $slot1 }

journal.window.backup.copy.offload.messages.degraded = etwas vom Datensatz dessen, was in deiner Sicherung ist, war nicht lesbar. diese Tage können mehr halten, als angezeigt wird.

journal.window.backup.copy.offload.messages.empty.days = noch keine ausgelagerten Medien.

journal.window.backup.copy.offload.messages.incomplete = einige Summen der Medien sind nicht verfügbar.

journal.window.backup.copy.offload.messages.saved = gespeichert

journal.window.backup.copy.offload.not.ready = schalte verschlüsselte Sicherung ein und bestätige deinen Wiederherstellungsschlüssel vor der Medienauslagerung.

journal.window.backup.copy.offload.paused.enable.note = setze die Sicherung fort, um die Medienauslagerung einzuschalten.

journal.window.backup.copy.offload.paused.note = die Medienauslagerung ist mit der Sicherung pausiert. keine Medien verlassen diesen Computer, bis du fortsetzt.

journal.window.backup.copy.offload.preparation.checking = Vorbereitung wird geprüft…

journal.window.backup.copy.offload.preparation.failed = Vorbereitung braucht Aufmerksamkeit. sieh dir das letzte Ergebnis der Auslagerung an.

journal.window.backup.copy.offload.preparation.finished = sieh dir das letzte Ergebnis der Auslagerung und die vorgeschlagenen Originale an.

journal.window.backup.copy.offload.preparation.pending = ein endgültiges Ergebnis ist nicht verfügbar. erneut prüfen.

journal.window.backup.copy.offload.preparation.unavailable = die Vorbereitung lässt sich gerade nicht bestätigen. erneut prüfen, bevor du eine weitere startest.

journal.window.backup.copy.offload.prepare.hint = bereite verschlüsselte Kopien zur Prüfung vor. wähle auf der Seite Originale, welche Originale du entfernst.

journal.window.backup.copy.offload.reason.unavailable = der Grund ist hier nicht verfügbar.

journal.window.backup.copy.offload.restore.reason.labels.archive.unbound = der Sicherungsdatensatz nennt das Archiv zum Wiederherstellen dieser Dateien nicht.

journal.window.backup.copy.offload.restore.reason.labels.auth.failed = verschlüsselte Sicherung hat Wiederherstellungsschlüssel oder Zugangsdaten abgelehnt.

journal.window.backup.copy.offload.restore.reason.labels.backup.not.ready = die verschlüsselte Sicherung ist nicht bereit, Medien wiederherzustellen.

journal.window.backup.copy.offload.restore.reason.labels.failed = Medienwiederherstellung konnte nicht abgeschlossen werden.

journal.window.backup.copy.offload.restore.reason.labels.insufficient.free.space = dieser Computer braucht mehr freien Speicherplatz, bevor Medien wiederhergestellt werden.

journal.window.backup.copy.offload.restore.reason.labels.ledger.degraded = ein Teil des Sicherungsdatensatzes ließ sich nicht lesen, der Vollständigkeit einer Wiederherstellung ist nicht zu vertrauen. versuch es nach der nächsten Sicherung erneut.

journal.window.backup.copy.offload.restore.reason.labels.legacy.read.failed = Sicherungskopie ausgelagerter Medien einer früheren Version deines Journals ließ sich nicht lesen und wurde nicht wiederhergestellt.

journal.window.backup.copy.offload.restore.reason.labels.legacy.review.held = Medien, die eine frühere Version deines Journals auslagerte, ließen sich nicht gegen deine Sicherung prüfen und wurden nicht wiederhergestellt.

journal.window.backup.copy.offload.restore.reason.labels.legacy.verification.failed = die Sicherungskopie ausgelagerter Medien einer früheren Version deines Journals passte nicht zu ihrem Datensatz und wurde nicht wiederhergestellt.

journal.window.backup.copy.offload.restore.reason.labels.locked = die Medienwiederherstellung wartet, bis die Sicherungswartung fertig ist.

journal.window.backup.copy.offload.restore.reason.labels.missing.file.after.restore = Medienwiederherstellung wurde fertig, aber eine Datei fehlte noch.

journal.window.backup.copy.offload.restore.reason.labels.nothing.to.restore = für diesen Tag gibt es nichts wiederherzustellen.

journal.window.backup.copy.offload.restore.reason.labels.repo.missing = die verschlüsselte Sicherung konnte das Repository nicht finden.

journal.window.backup.copy.offload.restore.reason.labels.restore.conflict = eine andere Datei liegt schon dort, wo ein Teil dieser Medien hingehört, deshalb wurde die Datei dort behalten und die Sicherungskopie nicht wiederhergestellt.

journal.window.backup.copy.offload.restore.reason.labels.restore.held = das Wiederherstellen ausgelagerter Dateien ist vorübergehend nicht verfügbar.

journal.window.backup.copy.offload.restore.reason.labels.restore.not.published = einige Medien ließen sich diesmal nicht in dein Journal zurückbringen, und an ihre Stelle kam nichts. versuch es später noch einmal.

journal.window.backup.copy.offload.restore.reason.labels.segment.missing = dieser Tag ist lokal nicht mehr verfügbar.

journal.window.backup.copy.offload.restore.reason.labels.timeout = Medienwiederherstellung dauerte zu lange. versuch es später noch einmal.

journal.window.backup.copy.offload.restore.reason.labels.verification.failed = wiederhergestellte Medien passten nicht zur Prüfsumme der Sicherung.

journal.window.backup.copy.offload.restore.report.dispositions.already.present = schon auf diesem Computer

journal.window.backup.copy.offload.restore.report.dispositions.conflict = vorhandene Datei behalten

journal.window.backup.copy.offload.restore.report.dispositions.published = wiederhergestellt

journal.window.backup.copy.offload.restore.report.dispositions.unattempted = nicht versucht

journal.window.backup.copy.offload.restore.report.dispositions.unverified = Ergebnis unbestätigt

journal.window.backup.copy.offload.restore.report.files = Dateiergebnisse

journal.window.backup.copy.offload.restore.report.held = braucht noch Aufmerksamkeit

journal.window.backup.copy.offload.result.labels.error = nicht gelungen

journal.window.backup.copy.offload.result.labels.ok = abgeschlossen

journal.window.backup.copy.offload.result.labels.partial = teilweise abgeschlossen

journal.window.backup.copy.offload.result.labels.skipped = nicht gelaufen

journal.window.backup.copy.offload.result.labels.stalled = nicht abgeschlossen

journal.window.backup.copy.offload.result.labels.unknown = Ergebnis nicht verfügbar

journal.window.backup.copy.offload.stakes = danach hält deine Sicherung die einzige Kopie deiner älteren Tage. verlierst du deinen Wiederherstellungsschlüssel, holt sie niemand zurück, nicht einmal { $slot1 }.

journal.window.backup.copy.offload.stall.reason.labels.archive.failed = die Medienauslagerung konnte ältere Medien nicht zur verschlüsselten Sicherung hinzufügen.

journal.window.backup.copy.offload.stall.reason.labels.backup.failing = verschlüsselte Sicherung braucht eine intakte aktuelle Kopie, bevor die Medienauslagerung laufen kann.

journal.window.backup.copy.offload.stall.reason.labels.backup.not.ready = verschlüsselte Sicherung muss die Einrichtung beenden, bevor die Medienauslagerung laufen kann.

journal.window.backup.copy.offload.stall.reason.labels.confirm.failed = die Medienauslagerung konnte die gesicherten Medien nicht überprüfen.

journal.window.backup.copy.offload.stall.reason.labels.confirm.tool.failed = die Medienauslagerung konnte das Werkzeug zur Überprüfung nicht ausführen.

journal.window.backup.copy.offload.stall.reason.labels.locked = die Medienauslagerung wartet, bis die Sicherungswartung fertig ist.

journal.window.backup.copy.offload.stall.reason.labels.unexpected.error = Medienauslagerung ist unerwartet gestoppt. versuch es nach der Sicherungswartung noch einmal.

journal.window.backup.copy.offload.stall.reason.labels.verification.failed = die Überprüfung der Sicherung ist nicht gelungen. die Medienauslagerung wartet auf eine intakte Überprüfung.

journal.window.backup.copy.offload.stall.reason.labels.verification.missing = die Überprüfung der Sicherung muss laufen, bevor die Medienauslagerung starten kann.

journal.window.backup.copy.offload.stall.reason.labels.verification.overdue = die Überprüfung der Sicherung ist überfällig. die Medienauslagerung wartet auf eine neue Überprüfung.

journal.window.backup.copy.offload.stalled.lead = die Auslagerung konnte nicht abgeschlossen werden.

journal.window.backup.copy.offload.title = Medienauslagerung

journal.window.backup.copy.offload.unavailable.lead = kann den Auslagerungsstatus gerade nicht lesen.

journal.window.backup.copy.offload.unsupported = Medienauslagerung ist auf diesem Gerät nicht verfügbar.

journal.window.backup.copy.offload.verification.integrity.failed = die Sicherungskopie hat die Integritätsprüfung nicht bestanden und könnte beschädigt sein.

journal.window.backup.copy.operation.reason.labels.auth.failed = dieser Wiederherstellungsschlüssel hat die Sicherung nicht entsperrt. prüfe zuerst den Schlüssel, dann die Angaben zum Ziel.

journal.window.backup.copy.operation.reason.labels.backup.busy = eine andere Sicherungsaufgabe läuft schon. versuch es in einem Moment noch einmal.

journal.window.backup.copy.operation.reason.labels.backup.not.confirmed = bestätige deinen Wiederherstellungsschlüssel vor dem Einschalten der Sicherung.

journal.window.backup.copy.operation.reason.labels.backup.only.held = deine Sicherung hält die einzige Kopie einiger deiner Medien. nichts wurde an ihr geändert. stelle zuerst alles wieder her, dann versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.backup.only.unplaced = deine Sicherung hält die einzige Kopie einiger älterer Medien ohne passende Aufnahme auf diesem Computer. sie lassen sich nicht wiederherstellen, nichts an der Sicherung geändert. wurde dein Journal verschoben oder kopiert, hol die fehlenden Tage zurück, versuch es erneut.

journal.window.backup.copy.operation.reason.labels.backup.operation.failed = diese Sicherungsaktion ließ sich nicht abschließen. prüfe Wiederherstellungsschlüssel und Ziel, dann versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.backup.paused = Sicherung ist pausiert. setze sie fort, dann versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.backup.unavailable = der Sicherungsdienst war nicht erreichbar. starte ihn, dann versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.body.rebuild.failed = dein Journal wurde wiederhergestellt, der Körperverlauf ließ sich daraus nicht neu aufbauen. die Wiederherstellung nicht fertiggestellt.

journal.window.backup.copy.operation.reason.labels.broker.error = verschlüsselte Sicherung lieferte keine nutzbaren Einstellungen. versuch es gleich noch einmal.

journal.window.backup.copy.operation.reason.labels.broker.unreachable = die verschlüsselte Sicherung war nicht erreichbar. prüfe deine Verbindung, dann versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.cleanup.pending = es ließ sich nicht bestätigen, dass die Sicherungsaufgabe gestoppt ist. andere Sicherungsaktionen warten.

journal.window.backup.copy.operation.reason.labels.destination.unsupported = deine Sicherungseinstellungen sind unvollständig, so erreicht dein Journal sie nicht, und an der Sicherung wurde nichts geändert.

journal.window.backup.copy.operation.reason.labels.expired = die Freigabe hat zu lange gedauert. versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.failed = die Sicherungsaktion ließ sich nicht abschließen. prüfe Wiederherstellungsschlüssel und Ziel, dann versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.hosted.entitlement.inactive = schalte die verschlüsselte Sicherung im Diensteportal ein, dann versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.incomplete = die Sicherungsaktion wurde nicht fertig. du kannst es noch einmal versuchen.

journal.window.backup.copy.operation.reason.labels.integrity.failed = dein Journal auf diesem Computer wiederhergestellt. Sicherungskopie hat die Integritätsprüfung nicht bestanden und könnte beschädigt sein.

journal.window.backup.copy.operation.reason.labels.integrity.unverified = dein Journal wurde auf diesen Computer wiederhergestellt, die Integritätsprüfung lief nicht (die Sicherung war beschäftigt oder die Zeit war um). Sicherungskopie nicht überprüft.

journal.window.backup.copy.operation.reason.labels.invalid.config.value = verwend nichtnegative ganze Zahlen, dann speichere erneut.

journal.window.backup.copy.operation.reason.labels.invalid.key = dieser Wiederherstellungsschlüssel entsperrt die Sicherung nicht. gib ihn aus deiner gespeicherten Kopie erneut ein.

journal.window.backup.copy.operation.reason.labels.invalid.operation.for.state = beende diesen Einrichtungsschritt der Sicherung, dann versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.invalid.request.value = prüfe die Angaben zum Ziel und versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.locked = das Ziel ist beschäftigt. versuch es gleich noch einmal.

journal.window.backup.copy.operation.reason.labels.malformed = die Antwort konnte nicht gelesen werden. aktualisiere dein Journal, dann versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.missing.required.field = füll die Pflichtfelder aus, dann versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.network.error = das Diensteportal war nicht erreichbar. prüfe deine Verbindung, dann versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.offload.record.unavailable = der Datensatz, was die Auslagerung in deine Sicherung verschoben hat, fehlt oder ist unlesbar. nichts an der Sicherung geändert. wurde dein Journal verschoben oder kopiert, hol seinen Zustandsordner zurück und versuch es erneut.

journal.window.backup.copy.operation.reason.labels.recovery.key.mismatch = das passt nicht zu deinem Wiederherstellungsschlüssel. gib ihn aus deiner gespeicherten Kopie erneut ein.

journal.window.backup.copy.operation.reason.labels.repo.missing = an diesem Ziel wurde kein Sicherungs-Repository gefunden.

journal.window.backup.copy.operation.reason.labels.repository.unsupported = das Repository muss mit { $slot1 } oder { $slot2 } beginnen, passend zum backend, das du gewählt hast (zum Beispiel { $slot3 } oder { $slot4 }).

journal.window.backup.copy.operation.reason.labels.restore.prepare.expired = die Freigabe hat zu lange gedauert. versuch es noch einmal.

journal.window.backup.copy.operation.reason.labels.storage.unreached = das Ausschalten und Löschen deiner Sicherung ist gestoppt, bevor etwas gelöscht wurde, deshalb ist ein erneuter Versuch sicher.

journal.window.backup.copy.operation.reason.labels.teardown.tool.unavailable = das Sicherungsprogramm, das zu deinem Journal gehört, fehlt, deshalb wurde an der Sicherung nichts geändert.

journal.window.backup.copy.operation.reason.labels.timeout = das Ziel hat zu lange gebraucht, um zu antworten. versuch es gleich noch einmal.

journal.window.backup.copy.phase.labels.cleanup.pending = Sicherung braucht Aufmerksamkeit

journal.window.backup.copy.phase.labels.degraded = wiederhergestellt, aber nicht überprüft

journal.window.backup.copy.phase.labels.done = fertig

journal.window.backup.copy.phase.labels.empty = noch nicht eingerichtet

journal.window.backup.copy.phase.labels.error = nicht abgeschlossen

journal.window.backup.copy.phase.labels.loading = wird geladen…

journal.window.backup.copy.phase.labels.refused = Wiederherstellung war nicht verfügbar

journal.window.backup.copy.phase.labels.restoring = dein Journal wird wiederhergestellt…

journal.window.backup.copy.phase.labels.rotating = neuer Wiederherstellungsschlüssel wird erstellt…

journal.window.backup.copy.phase.labels.setting.up = deine Sicherung wird eingerichtet…

journal.window.backup.copy.phase.labels.tearing.down = wird ausgeschaltet…

journal.window.backup.copy.restore.expectation = eine große Wiederherstellung kann eine Weile dauern. du kannst diese Seite offen lassen, während sie läuft.

journal.window.backup.copy.restore.hosted.byo.desc = Speicher, den du selbst mitbringst, erreicht mit Zugangsdaten, die du bereitstellst.

journal.window.backup.copy.restore.hosted.choose.lane = wo ist die verschlüsselte Kopie, aus der du wiederherstellst?

journal.window.backup.copy.restore.hosted.errors.auth.failed = dieser Wiederherstellungsschlüssel entsperrt die Sicherung nicht. prüfe den Schlüssel, dann melde dich erneut an.

journal.window.backup.copy.restore.hosted.errors.cancelled = Wiederherstellung abgebrochen. du kannst es erneut versuchen.

journal.window.backup.copy.restore.hosted.key.label = dein Wiederherstellungsschlüssel

journal.window.backup.copy.restore.hosted.key.reassurance = dieses Journal nutzt deinen Schlüssel und sendet ihn nie an { $slot1 }.

journal.window.backup.copy.restore.hosted.key.required = gib deinen Wiederherstellungsschlüssel ein, bevor du fortfährst.

journal.window.backup.copy.restore.hosted.lane.intro = gib deinen Wiederherstellungsschlüssel ein, melde dich im Diensteportal an und bestätige die Wiederherstellung.

journal.window.backup.copy.restore.hosted.lane.title = wiederherstellen von { $slot1 }

journal.window.backup.copy.restore.hosted.operated.desc = Speicher, den { $slot1 } betreibt, erreicht über das Diensteportal.

journal.window.backup.copy.restore.hosted.popup.preflight.failed = das Anmeldungsfenster hat sich nicht geöffnet. versuch es noch einmal und prüfe, ob dein Browser es blockiert hat.

journal.window.backup.copy.restore.hosted.primary = anmelden zum Wiederherstellen →

journal.window.backup.copy.restore.hosted.reopen = Anmeldung erneut öffnen

journal.window.backup.copy.restore.hosted.state.b = wartet auf deine Freigabe der Wiederherstellung im Diensteportal…

journal.window.backup.copy.restore.hosted.state.b.refused.hosted.backup.expired = { $slot1 } hat diese Kopie gelöscht, 30 Tage nachdem die verschlüsselte Sicherung gestoppt ist.

journal.window.backup.copy.restore.hosted.state.b.refused.no.hosted.backup = { $slot1 } hält keine verschlüsselte Kopie für die Anmeldung, die du verwendet hast.

journal.window.backup.copy.restore.hosted.status.retryable = da ist etwas schiefgelaufen. du kannst es noch einmal versuchen.

journal.window.backup.copy.service.name = verschlüsselte Sicherung

journal.window.backup.copy.status.last.backup.never.run = keine abgeschlossene Sicherung auf diesem Computer verzeichnet

journal.window.backup.detail.bin.directory = aktuelle Programmdatei ist nicht in einem { $bin_directory }-Verzeichnis

journal.window.backup.detail.bin.parent = aktuelle Programmdatei hat kein übergeordnetes Verzeichnis

journal.window.backup.detail.package.root = Stammverzeichnis des Pakets nicht gefunden

journal.window.backup.detail.path.line.break = Journal-Pfad enthält einen Zeilenumbruch

journal.window.backup.erasure.paused.count = { $things_quantity ->
        [one] 1 endgültig gelöschte Sache wartet darauf, aus deinen Sicherungen entfernt zu werden, bis du fortsetzt.
       *[other] { $things } endgültig gelöschte Sachen warten darauf, aus deinen Sicherungen entfernt zu werden, bis du fortsetzt.
    }

journal.window.backup.erasure.running.count = { $things_quantity ->
        [one] 1 endgültig gelöschte Sache wird aus deinen Sicherungen entfernt…
       *[other] { $things } endgültig gelöschte Sachen werden aus deinen Sicherungen entfernt…
    }

journal.window.backup.link.label.backup.details.in.health = Sicherungsdetails im Zustand →

journal.window.backup.notice.failure.auth.failed = der Schlüssel deines Journals konnte die Sicherung an deinem Sicherungsziel nicht entsperren. öffne die Sicherung zum Prüfen.

journal.window.backup.notice.failure.binding.invalid = Verknüpfung deines Journals zur verschlüsselten Sicherung abgelehnt. öffne die Sicherung für Einzelheiten.

journal.window.backup.notice.failure.broker.unreachable = die verschlüsselte Sicherung war nicht erreichbar. öffne die Sicherung für Einzelheiten.

journal.window.backup.notice.failure.hosted.entitlement.inactive = die verschlüsselte Sicherung ist im Diensteportal nicht eingeschaltet. öffne die Sicherung für Einzelheiten.

journal.window.backup.notice.failure.locked = dein Sicherungsziel war mit der Sicherungswartung beschäftigt. öffne die Sicherung für Einzelheiten.

journal.window.backup.notice.failure.rclone.unavailable = der verschlüsselten Sicherung fehlt ein Teil, den sie braucht, wo dein Journal läuft. öffne die Sicherung für Einzelheiten.

journal.window.backup.notice.failure.repo.missing = an deinem Sicherungsziel wurde keine Sicherung gefunden. öffne die Sicherung, um sie zu prüfen.

journal.window.backup.notice.failure.repository.unsupported = das Repository in deinen Sicherungseinstellungen passt nicht zu seinem Speicher. öffne die Sicherung, um es zu ändern.

journal.window.backup.notice.failure.restic.unavailable = die verschlüsselte Sicherung kann nicht laufen, wo dein Journal läuft. öffne die Sicherung für Einzelheiten.

journal.window.backup.notice.failure.timeout = dem letzten Versuch ist die Zeit ausgegangen. öffne die Sicherung für Einzelheiten.

journal.window.backup.notice.failure.unknown = der letzte Versuch ist mit einem Fehler gestoppt. öffne die Sicherung für Einzelheiten.

journal.window.backup.notice.recovery.complete = der letzte Sicherungsversuch ist fertig. öffne die Sicherung für Einzelheiten.

journal.window.backup.notice.recovery.incomplete = der letzte Sicherungsversuch ist fertig, aber ein Teil deines Journals ließ sich nicht lesen. öffne die Sicherung für Einzelheiten.

journal.window.backup.notice.title.cleared = Sicherung funktioniert wieder

journal.window.backup.notice.title.raised = Sicherung ist wiederholt nicht gelungen

journal.window.backup.offload.capacity.suggested = vorgeschlagen: { $suggested_size }

journal.window.backup.offload.days.show.all = { $days_quantity ->
        [one] { $days } Tag anzeigen
       *[other] alle { $days } Tage anzeigen
    }

journal.window.backup.offload.restore.capacity.expectation = { $restore_size } wird aus deiner Sicherung wiederhergestellt. eine große Wiederherstellung kann eine Weile dauern.

journal.window.backup.offload.restore.day.accessible.name = diesen Tag wiederherstellen: { $day }

journal.window.backup.offload.restore.effects = { $files_quantity ->
        [one] Datei wiederhergestellt: { $files } · auf diesem Computer hinzugefügt: { $size }
       *[other] Dateien wiederhergestellt: { $files } · auf diesem Computer hinzugefügt: { $size }
    }

journal.window.backup.response.backup.refusal.a.backup.can.t.start.because = eine Sicherung kann nicht starten, weil der Hintergrunddienst deines Journals nicht läuft. starte ihn, dann versuch es noch einmal.

journal.window.backup.response.backup.refusal.a.backup.couldn.t.be.set = eine Sicherung konnte dort nicht eingerichtet werden. nichts wurde gespeichert.

journal.window.backup.response.backup.refusal.a.required.field.is.missing = ein Pflichtfeld fehlt.

journal.window.backup.response.backup.refusal.backup.can.t.be.turned.on = Sicherung lässt sich nicht einschalten, bis du deinen Wiederherstellungsschlüssel bestätigst.

journal.window.backup.response.backup.refusal.detail.backup.is.disabled = Sicherung ist deaktiviert

journal.window.backup.response.backup.refusal.detail.backup.is.off = Sicherung ist aus

journal.window.backup.response.backup.refusal.detail.backup.is.paused = Sicherung ist pausiert

journal.window.backup.response.backup.refusal.detail.backup.keys.are.missing = Sicherungsschlüssel fehlen

journal.window.backup.response.backup.refusal.detail.configure.a.destination.first = konfiguriere zuerst ein Ziel

journal.window.backup.response.backup.refusal.detail.hosted.backup.has.no.destination.to = gehostete Sicherung hat kein Ziel zum Ändern

journal.window.backup.response.backup.refusal.detail.invalid.region = ungültige Region

journal.window.backup.response.backup.refusal.detail.media.offload.is.not.available.on = Medienauslagerung ist auf diesem Gerät nicht verfügbar

journal.window.backup.response.backup.refusal.detail.no.recovery.key.yet = noch kein Wiederherstellungsschlüssel

journal.window.backup.response.backup.refusal.detail.nothing.was.saved = nichts wurde gespeichert

journal.window.backup.response.backup.refusal.detail.own.storage.backup.isn.t.on = Sicherung auf eigenem Speicher ist nicht an

journal.window.backup.response.backup.refusal.detail.repository.must.start.with.the.backend = Repository muss mit dem backend-Namen beginnen

journal.window.backup.response.backup.refusal.detail.unsupported.backend = nicht unterstütztes backend

journal.window.backup.response.backup.refusal.detail.use.the.hosted.restore.prepare.flow = nutze den Ablauf zum Vorbereiten der gehosteten Wiederherstellung

journal.window.backup.response.backup.refusal.one.of.those.values.couldn.t = einer dieser Werte ließ sich nicht verwenden.

journal.window.backup.response.backup.refusal.restore.couldn.t.start = Wiederherstellung konnte nicht starten.

journal.window.backup.response.backup.refusal.that.action.isn.t.available.in = diese Aktion ist im aktuellen Zustand nicht verfügbar.

journal.window.backup.response.backup.refusal.that.day.couldn.t.be.used = dieser Tag ließ sich nicht verwenden.

journal.window.backup.response.backup.refusal.that.didn.t.match.your.recovery = das hat nicht zu deinem Wiederherstellungsschlüssel gepasst.

journal.window.backup.response.backup.refusal.that.request.didn.t.finish = diese Anfrage wurde nicht fertig.

journal.window.backup.response.backup.refusal.that.s.a.different.repository.confirm = das ist ein anderes Repository. bestätige die Änderung, um es zu verwenden.

journal.window.backup.response.backup.refusal.that.s.already.your.destination = das ist schon dein Ziel.

journal.window.backup.response.backup.refusal.that.setting.couldn.t.be.saved = diese Einstellung konnte nicht gespeichert werden, weil ein Wert ungültig war.

journal.window.backup.response.backup.refusal.the.restore.from.encrypted.backup.couldn = Wiederherstellung aus verschlüsselter Sicherung konnte nicht vorbereitet werden.

journal.window.backup.response.backup.refusal.the.restore.from.encrypted.backup.didn = die Wiederherstellung aus der verschlüsselten Sicherung wurde nicht fertig.

journal.window.backup.response.backup.refusal.this.journal.already.has.a.hosted = Dieses Journal hat schon eine Bindung für die gehostete Sicherung.

journal.window.backup.response.backup.refusal.this.journal.must.start.its.hosted = Dieses Journal muss zuerst seine Übergabe der gehosteten Wiederherstellung starten.

journal.window.backup.response.destination.result.message.a.backup.there.doesn.t.open = eine Sicherung dort lässt sich mit dem Schlüssel dieses Journals nicht öffnen

journal.window.backup.response.destination.result.message.backup.destination.is.reachable.and.needs = Sicherungsziel ist erreichbar und muss eingerichtet werden

journal.window.backup.response.destination.result.message.backup.repository.is.reachable = Sicherungs-Repository ist erreichbar

journal.window.backup.response.destination.result.message.could.not.reach.the.backup.destination = konnte das Sicherungsziel nicht erreichen

journal.window.backup.response.destination.result.message.repository.is.locked.try.again.shortly = Repository ist gesperrt; versuch es gleich noch einmal

journal.window.backup.response.destination.result.message.repository.password.was.rejected = Repository-Passwort wurde abgelehnt

journal.window.backup.response.destination.result.message.the.destination.rejected.the.credentials = das Ziel hat die Zugangsdaten abgelehnt

journal.window.backup.response.destination.result.message.these.credentials.can.t.open.a = diese Zugangsdaten können keinen Bucket mit diesem Namen öffnen

journal.window.backup.response.detail.missing.field = fehlt{ $slot1 }{ $name }

journal.window.backup.response.detail.missing.request.body = Anfragetext fehlt

journal.window.backup.response.hosted.restore.handoff.refusal.detail.a.hosted.restore.handoff.is.already = Eine Übergabe der gehosteten Wiederherstellung wird schon vorbereitet.

journal.window.backup.response.hosted.restore.handoff.refusal.detail.that.hosted.restore.handoff.capability.has = Diese Fähigkeit für die gehostete Wiederherstellungsübergabe ist abgelaufen.

journal.window.backup.response.hosted.restore.handoff.refusal.detail.that.hosted.restore.handoff.capability.is = Diese Fähigkeit für die gehostete Wiederherstellungsübergabe ist nicht gültig.

journal.window.backup.response.hosted.restore.handoff.refusal.detail.that.hosted.restore.handoff.is.not = Diese gehostete Wiederherstellungsübergabe ist für diesen Schritt nicht bereit.

journal.window.backup.response.hosted.restore.handoff.refusal.detail.the.hosted.restore.handoff.is.no = Die gehostete Wiederherstellungsübergabe ist nicht mehr aktuell.

journal.window.backup.response.offload.change.partial = Änderung der Medienauslagerung ist nicht gelungen; lade neu, bevor du es noch einmal versuchst.

journal.window.backup.response.pause.message = Sicherung ist pausiert. setz die Sicherung zuerst fort.

journal.window.backup.response.storage.retention.result.your.b2.bucket.only.hides.deleted = dein B2-Bucket verbirgt gelöschte Dateien nur, außer eine Lebenszyklusregel für den ganzen Bucket entfernt sie, daher bleiben Kopien dieser Sicherung dort, bis du eine hinzufügst oder sie selbst löschst.

journal.window.backup.response.storage.retention.result.your.bucket.can.lock.files.against = dein Bucket kann Dateien gegen Löschung sperren, daher bleiben Kopien dieser Sicherung dort, bis jede Sperre endet und du diese Versionen löschst oder die eigenen Regeln deines Anbieters sie entfernen.

journal.window.backup.response.storage.retention.result.your.journal.couldn.t.check.whether = dein Journal konnte nicht prüfen, ob dein Speicher gelöschte Dateien behält, prüfe also Versionierung, Lebenszyklus und Sperre deines Buckets, damit du sicher bist, dass diese Sicherung weg ist.

journal.window.backup.response.storage.retention.result.your.storage.keeps.earlier.versions.of = dein Speicher behält frühere Versionen gelöschter Dateien, daher bleiben Kopien dieser Sicherung in deinem Bucket, bis du diese Versionen löschst oder die eigenen Regeln deines Anbieters sie entfernen.

journal.window.backup.response.tool.manifest.invalid = ein Teil der Installation deines Journals ist beschädigt; installiere dein Journal neu.

journal.window.backup.response.tool.manifest.missing = ein Teil der Installation deines Journals fehlt; installiere dein Journal neu.

journal.window.backup.response.tool.member.unreadable = ein Teil der Installation deines Journals konnte nicht gelesen werden; wenn es weiter passiert, installiere dein Journal neu.

journal.window.backup.response.tool.package.mismatch = die Installation deines Journals passt nicht; wenn du gerade aktualisiert hast, starte dein Journal neu, sonst installiere es neu.

journal.window.backup.response.tool.restart.update = starte dein Journal neu, um die Aktualisierung abzuschließen.

journal.window.backup.response.tool.screen.ocr.unavailable = Bildschirm-OCR ist auf diesem Zielsystem nicht verfügbar.

journal.window.backup.response.tool.unsafe.path = ein Teil der Installation deines Journals ist nicht so, wie er sein sollte; installiere dein Journal neu.

journal.window.backup.response.tool.unsupported.location = diese Kopie des Journals läuft nicht aus einem installierten Paket, daher können deren Dateien nicht geprüft werden.

journal.window.backup.response.tool.wrong.product = was hier installiert ist, ist nicht das Journal; installiere dein Journal neu.

journal.window.backup.response.tool.wrong.target = dein Journal wurde für eine andere Art von Computer gebaut; installiere die Version für diesen neu.

journal.window.backup.status.activity.active.label = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: läuft
        [rotate] Änderung des Wiederherstellungsschlüssels: läuft
        [stop] Ausschalten der Sicherung: läuft
        [restore] Wiederherstellung: läuft
        [prepare] Vorbereitung der Kopie: läuft
       *[approve] Freigabe des Archivs: läuft
    }

journal.window.backup.status.activity.active.sentence = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: läuft.
        [rotate] Änderung des Wiederherstellungsschlüssels: läuft.
        [stop] Ausschalten der Sicherung: läuft.
        [restore] Wiederherstellung: läuft.
        [prepare] Vorbereitung der Kopie: läuft.
       *[approve] Freigabe des Archivs: läuft.
    }

journal.window.backup.status.activity.cancelled.label = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: abgebrochen
        [rotate] Änderung des Wiederherstellungsschlüssels: abgebrochen
        [stop] Ausschalten der Sicherung: abgebrochen
        [restore] Wiederherstellung: abgebrochen
        [prepare] Vorbereitung der Kopie: abgebrochen
       *[approve] Freigabe des Archivs: abgebrochen
    }

journal.window.backup.status.activity.cancelled.sentence = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: abgebrochen.
        [rotate] Änderung des Wiederherstellungsschlüssels: abgebrochen.
        [stop] Ausschalten der Sicherung: abgebrochen.
        [restore] Wiederherstellung: abgebrochen.
        [prepare] Vorbereitung der Kopie: abgebrochen.
       *[approve] Freigabe des Archivs: abgebrochen.
    }

journal.window.backup.status.activity.cancelling.label = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: wird abgebrochen
        [rotate] Änderung des Wiederherstellungsschlüssels: wird abgebrochen
        [stop] Ausschalten der Sicherung: wird abgebrochen
        [restore] Wiederherstellung: wird abgebrochen
        [prepare] Vorbereitung der Kopie: wird abgebrochen
       *[approve] Freigabe des Archivs: wird abgebrochen
    }

journal.window.backup.status.activity.cancelling.sentence = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: wird abgebrochen.
        [rotate] Änderung des Wiederherstellungsschlüssels: wird abgebrochen.
        [stop] Ausschalten der Sicherung: wird abgebrochen.
        [restore] Wiederherstellung: wird abgebrochen.
        [prepare] Vorbereitung der Kopie: wird abgebrochen.
       *[approve] Freigabe des Archivs: wird abgebrochen.
    }

journal.window.backup.status.activity.cleanup.label = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: wartet auf das Aufräumen
        [rotate] Änderung des Wiederherstellungsschlüssels: wartet auf das Aufräumen
        [stop] Ausschalten der Sicherung: wartet auf das Aufräumen
        [restore] Wiederherstellung: wartet auf das Aufräumen
        [prepare] Vorbereitung der Kopie: wartet auf das Aufräumen
       *[approve] Freigabe des Archivs: wartet auf das Aufräumen
    }

journal.window.backup.status.activity.cleanup.sentence = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: wartet auf das Aufräumen.
        [rotate] Änderung des Wiederherstellungsschlüssels: wartet auf das Aufräumen.
        [stop] Ausschalten der Sicherung: wartet auf das Aufräumen.
        [restore] Wiederherstellung: wartet auf das Aufräumen.
        [prepare] Vorbereitung der Kopie: wartet auf das Aufräumen.
       *[approve] Freigabe des Archivs: wartet auf das Aufräumen.
    }

journal.window.backup.status.activity.completed.label = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: abgeschlossen
        [rotate] Änderung des Wiederherstellungsschlüssels: abgeschlossen
        [stop] Ausschalten der Sicherung: abgeschlossen
        [restore] Wiederherstellung: abgeschlossen
        [prepare] Vorbereitung der Kopie: abgeschlossen
       *[approve] Freigabe des Archivs: abgeschlossen
    }

journal.window.backup.status.activity.degraded.label = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: mit Problemen abgeschlossen
        [rotate] Änderung des Wiederherstellungsschlüssels: mit Problemen abgeschlossen
        [stop] Ausschalten der Sicherung: mit Problemen abgeschlossen
        [restore] Wiederherstellung: mit Problemen abgeschlossen
        [prepare] Vorbereitung der Kopie: mit Problemen abgeschlossen
       *[approve] Freigabe des Archivs: mit Problemen abgeschlossen
    }

journal.window.backup.status.activity.degraded.sentence = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: mit Problemen abgeschlossen.
        [rotate] Änderung des Wiederherstellungsschlüssels: mit Problemen abgeschlossen.
        [stop] Ausschalten der Sicherung: mit Problemen abgeschlossen.
        [restore] Wiederherstellung: mit Problemen abgeschlossen.
        [prepare] Vorbereitung der Kopie: mit Problemen abgeschlossen.
       *[approve] Freigabe des Archivs: mit Problemen abgeschlossen.
    }

journal.window.backup.status.activity.failed.label = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: nicht gelungen
        [rotate] Änderung des Wiederherstellungsschlüssels: nicht gelungen
        [stop] Ausschalten der Sicherung: nicht gelungen
        [restore] Wiederherstellung: nicht gelungen
        [prepare] Vorbereitung der Kopie: nicht gelungen
       *[approve] Freigabe des Archivs: nicht gelungen
    }

journal.window.backup.status.activity.failed.sentence = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: nicht gelungen.
        [rotate] Änderung des Wiederherstellungsschlüssels: nicht gelungen.
        [stop] Ausschalten der Sicherung: nicht gelungen.
        [restore] Wiederherstellung: nicht gelungen.
        [prepare] Vorbereitung der Kopie: nicht gelungen.
       *[approve] Freigabe des Archivs: nicht gelungen.
    }

journal.window.backup.status.activity.refused.label = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: konnte nicht starten
        [rotate] Änderung des Wiederherstellungsschlüssels: konnte nicht starten
        [stop] Ausschalten der Sicherung: konnte nicht starten
        [restore] Wiederherstellung: konnte nicht starten
        [prepare] Vorbereitung der Kopie: konnte nicht starten
       *[approve] Freigabe des Archivs: konnte nicht starten
    }

journal.window.backup.status.activity.refused.sentence = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: konnte nicht starten.
        [rotate] Änderung des Wiederherstellungsschlüssels: konnte nicht starten.
        [stop] Ausschalten der Sicherung: konnte nicht starten.
        [restore] Wiederherstellung: konnte nicht starten.
        [prepare] Vorbereitung der Kopie: konnte nicht starten.
       *[approve] Freigabe des Archivs: konnte nicht starten.
    }

journal.window.backup.status.activity.subscription.label = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: wartet auf den Abschluss im Diensteportal
        [rotate] Änderung des Wiederherstellungsschlüssels: wartet auf den Abschluss im Diensteportal
        [stop] Ausschalten der Sicherung: wartet auf den Abschluss im Diensteportal
        [restore] Wiederherstellung: wartet auf den Abschluss im Diensteportal
        [prepare] Vorbereitung der Kopie: wartet auf den Abschluss im Diensteportal
       *[approve] Freigabe des Archivs: wartet auf den Abschluss im Diensteportal
    }

journal.window.backup.status.activity.subscription.sentence = { $operation_kind ->
        [setup] Ersteinrichtung der Sicherung: wartet auf den Abschluss im Diensteportal.
        [rotate] Änderung des Wiederherstellungsschlüssels: wartet auf den Abschluss im Diensteportal.
        [stop] Ausschalten der Sicherung: wartet auf den Abschluss im Diensteportal.
        [restore] Wiederherstellung: wartet auf den Abschluss im Diensteportal.
        [prepare] Vorbereitung der Kopie: wartet auf den Abschluss im Diensteportal.
       *[approve] Freigabe des Archivs: wartet auf den Abschluss im Diensteportal.
    }

journal.window.backup.status.backup.off = Sicherung ist aus.

journal.window.backup.status.backup.off.superseded = Sicherung ist aus. eine andere Kopie deines Journals hat die verschlüsselte Sicherung übernommen.

journal.window.backup.status.check.result.date.known.label = { $outcome ->
        [completed] abgeschlossen, { $recorded_at }
        [failed] nicht gelungen, { $recorded_at }
        [never-recorded] kein abgeschlossener Versuch verzeichnet, { $recorded_at }
       *[could-not-run] konnte nicht laufen, { $recorded_at }
    }

journal.window.backup.status.check.result.date.known.sentence = { $outcome ->
        [completed] abgeschlossen, { $recorded_at }.
        [failed] nicht gelungen, { $recorded_at }.
        [never-recorded] kein abgeschlossener Versuch verzeichnet, { $recorded_at }.
       *[could-not-run] konnte nicht laufen, { $recorded_at }.
    }

journal.window.backup.status.check.result.date.unknown.label = { $outcome ->
        [completed] abgeschlossen, Datum ist nicht bekannt
        [failed] nicht gelungen, Datum ist nicht bekannt
        [never-recorded] kein abgeschlossener Versuch verzeichnet, Datum ist nicht bekannt
       *[could-not-run] konnte nicht laufen, Datum ist nicht bekannt
    }

journal.window.backup.status.check.result.date.unknown.sentence = { $outcome ->
        [completed] abgeschlossen, Datum ist nicht bekannt.
        [failed] nicht gelungen, Datum ist nicht bekannt.
        [never-recorded] kein abgeschlossener Versuch verzeichnet, Datum ist nicht bekannt.
       *[could-not-run] konnte nicht laufen, Datum ist nicht bekannt.
    }

journal.window.backup.status.fact.last.check = letzte Stichprobe

journal.window.backup.status.fact.last.success = letzte erfolgreiche Kopie

journal.window.backup.status.fact.latest.attempt = letzter Versuch

journal.window.backup.status.fact.name = Sicherung

journal.window.backup.status.fact.none.reported = nichts gemeldet

journal.window.backup.status.fact.off = aus

journal.window.backup.status.fact.on = an

journal.window.backup.status.fact.operation = Vorgang

journal.window.backup.status.last.attempt.completed.date = der letzte Sicherungsversuch wurde abgeschlossen, { $completed_at }.

journal.window.backup.status.outcome.unknown = ist nicht bekannt

journal.window.backup.status.paused.label.date = von dir pausiert, { $paused_at }

journal.window.backup.status.paused.on.date = du hast die Sicherung am { $paused_at } pausiert.

journal.window.backup.status.visible.text.technical.details = technische Details

journal.window.backup.status.window.explanation.another.copy.of.your.journal.took = eine andere Kopie deines Journals hat die verschlüsselte Sicherung übernommen.

journal.window.backup.status.window.explanation.backup.is.on.the.latest.attempt = Sicherung ist an. der letzte Versuch ist nicht bekannt.

journal.window.backup.status.window.explanation.backup.status.is.unavailable.right.now = der Status der Sicherung ist gerade nicht verfügbar.

journal.window.backup.status.window.explanation.encrypted.backup.can.t.reach.storage = die verschlüsselte Sicherung erreicht den Speicher von diesem Computer aus nicht.

journal.window.backup.status.window.explanation.encrypted.backup.can.t.run.on = die verschlüsselte Sicherung kann auf diesem Computer nicht laufen.

journal.window.backup.status.window.explanation.encrypted.backup.couldn.t.be.reached = die verschlüsselte Sicherung war nicht erreichbar.

journal.window.backup.status.window.explanation.encrypted.backup.didn.t.return.usable = die verschlüsselte Sicherung hat keine nutzbaren Einstellungen zurückgegeben.

journal.window.backup.status.window.explanation.encrypted.backup.isn.t.turned.on = die verschlüsselte Sicherung ist im Diensteportal nicht eingeschaltet.

journal.window.backup.status.window.explanation.it.ran.out.of.time = die Zeit ist abgelaufen.

journal.window.backup.status.window.explanation.paused.by.you.date.isn.t = von dir pausiert, Datum ist nicht bekannt

journal.window.backup.status.window.explanation.some.of.your.journal.couldn.t = einiges von deinem Journal konnte nicht gelesen werden, daher ist die gespeicherte Kopie unvollständig.

journal.window.backup.status.window.explanation.the.backup.key.couldn.t.open = der Sicherungsschlüssel konnte das Ziel nicht öffnen.

journal.window.backup.status.window.explanation.the.backup.lock.was.already.held = die Sicherungssperre war bereits belegt.

journal.window.backup.status.window.explanation.the.backup.tool.stopped.for.a = das Sicherungswerkzeug ist aus einem Grund gestoppt, den es nicht genannt hat.

journal.window.backup.status.window.explanation.the.backup.tool.stopped.with.an = das Sicherungswerkzeug ist mit einem Fehler gestoppt.

journal.window.backup.status.window.explanation.the.last.recorded.backup.spot.check = die letzte verzeichnete Stichprobe der Sicherung ist nicht gelungen.

journal.window.backup.status.window.explanation.the.latest.recorded.backup.attempt.failed = der letzte verzeichnete Sicherungsversuch ist nicht gelungen.

journal.window.backup.status.window.explanation.the.latest.recorded.backup.copy.is = die letzte verzeichnete Sicherungskopie ist unvollständig.

journal.window.backup.status.window.explanation.the.reason.wasn.t.recorded = der Grund wurde nicht verzeichnet.

journal.window.backup.status.window.explanation.the.repository.in.your.backup.settings = das Repository deiner Sicherungseinstellungen beginnt nicht mit { $slot1 } oder { $slot2 } wie sein backend. Sicherung ist daher gestoppt. ändere das Ziel in ein Repository mit { $slot3 } oder { $slot4 }

journal.window.backup.status.window.explanation.these.are.recorded.results.they.don = das sind verzeichnete Ergebnisse. sie bestätigen nicht, was an deinem aktuellen Ziel liegt oder ob es überprüft wurde.

journal.window.backup.status.window.explanation.this.computer.can.t.set.up = dieser Computer kann keine verschlüsselte Sicherung einrichten oder wiederherstellen.

journal.window.backup.status.window.explanation.this.computer.s.link.to.encrypted = die Verbindung dieses Computers zur verschlüsselten Sicherung wurde abgelehnt.

journal.window.backup.status.window.explanation.you.paused.backup.the.date.isn = du hast die Sicherung pausiert. das Datum ist nicht bekannt.

journal.window.backup.status.window.explanation.your.backup.destination.couldn.t.be = dein Sicherungsziel wurde nicht gefunden.

journal.window.backup.storage.user.backend = dein eigener Speicher · { $storage_backend }

journal.window.backup.verification.parts.checked = { $parts_quantity ->
        [one] 1 von { $parts } Teil deiner Sicherung geprüft.
       *[other] 1 von { $parts } Teilen deiner Sicherung geprüft.
    }

journal.window.backup.verification.parts.checked.all = diese Woche deine ganze Sicherung geprüft.

journal.window.backup.visible.text.checking.backup.status = Status der Sicherung wird geprüft…

journal.window.backup.visible.text.couldn.t.load.backup.settings.reload = Sicherungseinstellungen waren nicht zu laden. lade diese Seite neu, versuch es noch einmal.

journal.window.backup.visible.text.loading.backup.settings = Sicherungseinstellungen werden geladen…

journal.window.backup.visible.text.older.files.moved.off.this.computer = ältere Dateien wurden von diesem Computer ausgelagert. deine ganze Sicherung enthält auch, was noch hier ist.

journal.window.backup.window.explanation.a.backup.waits.until.this.finishes = eine Sicherung wartet, bis das fertig ist. du kannst diese Seite verlassen.

journal.window.handoff.sme.already.enabled.detail = solstone.me ist bereits an

journal.window.handoff.sme.consent.link.prepare.failed.detail = Einschalten von solstone.me konnte nicht starten. versuch es noch einmal.

journal.window.handoff.sme.outcome.guidance.expired = die Anfrage wurde innerhalb von etwa fünfzehn Minuten nicht erlaubt. schalte solstone.me wieder ein, wenn du bereit bist.

journal.window.handoff.sme.outcome.guidance.local.error = dieser Computer konnte solstone.me nicht fertig einschalten. versuch es noch einmal.

journal.window.handoff.sme.outcome.guidance.malformed = services.solstone.app hat so geantwortet, wie dieser Computer es nicht erwartet hat. versuch es in Kürze noch einmal.

journal.window.handoff.sme.outcome.guidance.needs.subscription = schließ das Einschalten im Diensteportal ab, komm dann zurück und schalte solstone.me erneut ein.

journal.window.handoff.sme.outcome.guidance.network.error = services.solstone.app war nicht erreichbar. prüfe die Verbindung dieses Computers und versuch es noch einmal.

journal.window.handoff.sme.outcome.guidance.revoked = versuch es noch einmal, wenn du bereit bist.

journal.window.handoff.spl.outcome.guidance.expired = Dieser Aktivierungslink ist nicht mehr aktiv. Starte einen neuen Aktivierungsablauf.

journal.window.handoff.spl.outcome.guidance.local.error = Lokaler Dienstzustand ließ sich nicht schreiben. Prüfe die Journal-Berechtigungen und versuch es noch einmal.

journal.window.handoff.spl.outcome.guidance.malformed = Die Antwort des Dienstes wurde nicht verstanden. Aktualisiere solstone und versuch es noch einmal.

journal.window.handoff.spl.outcome.guidance.network.error = Der Dienst war nicht erreichbar. Prüfe den Netzwerkzugang und versuch es noch einmal.

journal.window.handoff.spl.outcome.guidance.pending = Lass die Freigabeseite offen, während die Anfrage fertig wird.

journal.window.handoff.spl.outcome.guidance.relay.identity.conflict = dieses Journal ist schon unter einer anderen Identität eingerichtet. wend dich an den Support, setze es zurück und versuch es noch einmal.

journal.window.handoff.spl.outcome.guidance.relay.rotation.unsupported = der Sicherheitsschlüssel dieses Journals änderte sich und ist noch nicht automatisch neu registrierbar. wend dich an den Support.

journal.window.handoff.spl.outcome.guidance.relay.unavailable = der Dienst für das private Netzwerk ist gerade nicht verfügbar. versuch es in Kürze noch einmal.

journal.window.handoff.spl.outcome.guidance.revoked = Einwilligung nicht erteilt. Starte einen neuen Aktivierungsablauf, wenn du bereit bist.

journal.window.handoff.spl.private.link.already.enabled.detail = dein privates Netzwerk ist bereits an

journal.window.handoff.spl.private.link.consent.link.prepare.failed.detail = der Einwilligungslink konnte nicht vorbereitet werden.

journal.window.handoff.spl.relay.rejected.with.status = das Relay konnte dein privates Netzwerk nicht fertig einrichten (Fehler { $http_status }).

journal.window.handoff.spp.expired = vertrauliche Verarbeitung ist nicht an. der Link zum Einschalten ist abgelaufen, bevor du im Browser fertig warst. schalte sie erneut ein, um von vorn zu beginnen.

journal.window.handoff.spp.local.error = vertrauliche Verarbeitung ist nicht an. dein Journal konnte die Einstellung nicht speichern. schalte sie erneut ein.

journal.window.handoff.spp.malformed = vertrauliche Verarbeitung ist nicht an. das Diensteportal sandte eine Antwort, die dein Journal nicht lesen konnte. schalte sie erneut ein. wenn es bleibt, aktualisiere dein Journal.

journal.window.handoff.spp.network.error = vertrauliche Verarbeitung ist nicht an. dein Journal konnte das Diensteportal nicht erreichen. prüfe die Verbindung des Computers, auf dem dein Journal läuft, und schalte sie dann erneut ein.

journal.window.handoff.spp.not.verified = Hardware-Attestierung ist noch nicht überprüft. Denken bleibt blockiert, bis die Überprüfung fertig ist.

journal.window.handoff.spp.pending = schließ das Einschalten in deinem Browser ab.

journal.window.handoff.spp.revoked = vertrauliche Verarbeitung ist nicht an. schalte sie wieder ein, wenn du bereit bist.

journal.window.import.accessible.label.notebook = Notizbuch

journal.window.import.accessible.name.choose.file.to.upload = Datei zum Hochladen wählen

journal.window.import.accessible.name.close = schließen

journal.window.import.accessible.name.filter.by.month = nach Monat filtern

journal.window.import.answer.yes = ja

journal.window.import.apple.export.too.large = dieser Export ist größer als 4.3 { $slot1 }, das meiste, was diese Seite hochlädt. führe stattdessen aus: { $slot2 }

journal.window.import.apple.words.cancel.cancel = abbrechen

journal.window.import.apple.words.keeps.running.this.keeps.running.if.you.close = das läuft weiter, wenn du diese Seite schließt. ein langer Verlauf kann eine Stunde oder mehr dauern.

journal.window.import.apple.words.reading.reading.your.export = dein Export wird gelesen…

journal.window.import.body.facet.apple.ask.make.body = leg eine Körperfacette für deine Trainings, deinen Schlaf und andere Körperdaten an?

journal.window.import.body.facet.apple.ask.named = leg deine Trainings, deinen Schlaf und andere Körperdaten unter der Facette ab, die du { $facet_name } genannt hast?

journal.window.import.body.facet.apple.ask.unmute = deine Körperfacette ist stummgeschaltet. schalte sie für deine Trainings, Schlaf und andere Körperdaten wieder ein?

journal.window.import.body.facet.apple.files.named = das legt alles in diesem Export unter { $facet_name } ab, auch was schon auf der Seite Aktivitäten steht. die Vorschau sagt, wie viele davon sich höchstens verschieben könnten.

journal.window.import.body.facet.apple.no.means = wenn du nein sagst, stehen neue Trainings und neuer Schlaf nicht auf der Seite Aktivitäten, und Agenten nur für manche Facetten sehen sie nicht. Suche und Körper-App zeigen sie weiter.

journal.window.import.body.facet.apple.none.hint = neue Trainings und neuer Schlaf in diesem Import stehen nicht auf der Seite Aktivitäten, und Agenten nur für manche Facetten sehen sie nicht. Suche und Körper-App zeigen sie weiter.

journal.window.import.body.facet.shared.answer.failed = deine Antwort konnte nicht gespeichert werden. versuch es noch einmal.

journal.window.import.body.facet.shared.ask.make.body = leg eine Körperfacette für deine Trainings an?

journal.window.import.body.facet.shared.ask.named = leg deine Trainings unter der Facette ab, die du { $facet_name } genannt hast?

journal.window.import.body.facet.shared.ask.unmute = deine Körperfacette ist stummgeschaltet. schalte sie für deine Trainings wieder ein?

journal.window.import.body.facet.shared.files.named = das legt jedes Training in diesem Download unter { $facet_name } ab, auch solche, die schon auf der Seite Aktivitäten stehen. die Vorschau sagt, wie viele davon sich verschieben würden.

journal.window.import.body.facet.shared.heading = Facette

journal.window.import.body.facet.shared.no = nein

journal.window.import.body.facet.shared.no.means = wenn du nein sagst, stehen neue Trainings nicht auf der Seite Aktivitäten, keine Personen oder Dinge kommen aus ihnen, und Agenten nur für manche Facetten sehen sie nicht. Suche und Körper-App zeigen sie weiter.

journal.window.import.body.facet.shared.none.hint = neue Trainings in diesem Import stehen nicht auf der Seite Aktivitäten, keine Personen oder Dinge kommen aus ihnen, und Agenten nur für manche Facetten sehen sie nicht. Suche und Körper-App zeigen sie weiter.

journal.window.import.body.facet.shared.none.option = keine Facette

journal.window.import.body.facet.shared.offer.changed = die Facettenfrage änderte sich, nachdem du sie gesehen hast, daher wurde deine Antwort nicht gespeichert. die Karte zeigt jetzt den Stand.

journal.window.import.body.facet.shared.this.import = Facette für diesen Import

journal.window.import.body.facet.shared.unanswered = beantworte zuerst die Facettenfrage, zeig die Vorschau dann erneut an.

journal.window.import.body.facet.shared.yes = ja

journal.window.import.body.history.busy = { $reason }{ $slot1 }sobald es fertig ist, importiere den Export erneut.

journal.window.import.body.history.carried = gestoppt, bevor es fertig war. weitergeführt von deinem Import von{ $slot1 }{ $time }.

journal.window.import.body.history.carried.running = gestoppt, bevor es fertig war. weitergeführt von deinem Import von{ $slot1 }{ $time }, der jetzt läuft.

journal.window.import.body.history.followed.done = gestoppt, bevor es fertig war. dein Import von{ $slot1 }{ $time }{ $slot2 }lief danach.

journal.window.import.body.history.followed.running = gestoppt, bevor es fertig war. dein Import von{ $slot1 }{ $time }{ $slot2 }läuft jetzt.

journal.window.import.body.history.partly.saved.many = { $count }{ $slot1 }Teile deines Journals wurden nicht fertig gespeichert. jeder erneute { $slot2 }-Import versucht sie.

journal.window.import.body.history.partly.saved.one = 1 Teil deines Journals wurde nicht fertig gespeichert. jeder erneute { $slot1 }-Import versucht ihn.

journal.window.import.body.history.partly.saved.unknown = einige Teile deines Journals wurden nicht fertig gespeichert. jeder erneute { $slot1 }-Import versucht sie.

journal.window.import.body.history.stopped.default = es wurde gestoppt, bevor es fertig war. importiere den Export erneut, um fertig zu werden.

journal.window.import.body.history.time.label = { $day }.{ $slot1 }{ $month },{ $slot2 }{ $clock }

journal.window.import.body.label.didnt.start = nicht gestartet

journal.window.import.body.label.failed = nicht hereingeholt

journal.window.import.body.label.partly.saved = teilweise gespeichert

journal.window.import.body.label.saved = gespeichert

journal.window.import.body.label.stopped = gestoppt

journal.window.import.body.phase.words.place.sorting.what.it.read = sortiert, was es gelesen hat

journal.window.import.body.phase.words.repair.checking.what.an.earlier.import.left = wird geprüft, was ein früherer Import hinterlassen hat

journal.window.import.body.phase.words.retry.trying.again.what.didn.t.save = versucht erneut, was nicht gespeichert wurde

journal.window.import.body.phase.words.scan.reading.your.export = dein Export wird gelesen

journal.window.import.body.phase.words.write.saving = wird gespeichert

journal.window.import.body.status.reason.busy = ein anderer { $slot1 }-Import lief bereits, daher ist dieser nicht gestartet.

journal.window.import.body.status.reason.disk.full = die Festplatte auf dem Computer, auf dem dein Journal läuft, ist vollgelaufen, bevor es fertig war.

journal.window.import.body.status.reason.export.incomplete = dieser Export wirkt unvollständig, daher wurde nichts davon gespeichert. exportiere ihn erneut aus { $slot1 } und importiere die neue Kopie.

journal.window.import.body.status.reason.export.too.large = dieser Export entpackt sich auf mehr als 68.7 { $slot1 }, das meiste, was ein Import liest.

journal.window.import.body.status.reason.export.unreadable = dieser Export konnte nicht gelesen werden. exportiere ihn erneut aus { $slot1 } und importiere die neue Kopie.

journal.window.import.body.status.reason.facet = die dafür gewählte Facette ist stummgeschaltet oder weg, daher wurde nichts hereingeholt.

journal.window.import.body.status.reason.go.ahead = es hatte deine Freigabe nicht, daher wurde nichts hereingeholt.

journal.window.import.body.status.reason.no.room = auf dem Computer, auf dem dein Journal läuft, war kein Platz, um zu starten, daher wurde nichts hereingeholt. schaff Platz und importiere den Export dann erneut.

journal.window.import.body.status.reason.not.started = es ist nicht gestartet, daher wurde nichts hereingeholt.

journal.window.import.body.status.reason.not.this.import = dieser Import hält jenen Export nicht, daher wurde nichts hereingeholt.

journal.window.import.body.status.reason.other = es wurde gestoppt, bevor es fertig war.

journal.window.import.body.status.reason.permission = das Journal durfte den Export nicht öffnen.

journal.window.import.body.status.reason.preview.again = die Facette, die Zeitzone oder der Export hat sich nach der Vorschau geändert, daher wurde nichts hereingeholt. importiere den Export erneut von der { $slot1 }-Karte.

journal.window.import.body.status.reason.whole.export = ein { $slot1 }-Import liest immer den ganzen Export, daher wurde nichts hereingeholt.

journal.window.import.body.status.reason.windows.not.saving = zu viele Teile deines Journals nacheinander konnten nicht gespeichert werden, daher wurde gestoppt.

journal.window.import.body.status.stop.disk = { $reason }{ $slot1 }schaff Platz und importiere den Export dann erneut.

journal.window.import.body.status.stop.resume = { $reason }{ $slot1 }importiere den Export erneut, um fertig zu werden.

journal.window.import.body.status.upload.limit.mac = dieser Export ist größer als 4.3 { $slot1 }, das meiste, was diese Seite hochlädt. wähle stattdessen in der Journal-App im Menü journal Admin-Terminal öffnen und führe aus: { $slot2 }

journal.window.import.body.status.upload.limit.other = dieser Export ist größer als 4.3 { $slot1 }, das meiste, was diese Seite hochlädt. führe stattdessen von einem Terminal auf dem Computer, auf dem dein Journal läuft, aus: { $slot2 }

journal.window.import.button.label.check.status = Status prüfen

journal.window.import.button.label.choose.folder = Ordner wählen

journal.window.import.button.label.next = weiter

journal.window.import.button.label.prev = vorige

journal.window.import.button.label.preview.import = Importvorschau

journal.window.import.button.label.validate = überprüfen

journal.window.import.column.heading.entries = Einträge

journal.window.import.column.heading.file = Datei

journal.window.import.column.heading.imported.at = importiert am

journal.window.import.column.heading.imported.to = importiert nach

journal.window.import.column.heading.source = Quelle

journal.window.import.column.heading.status = Status

journal.window.import.consent.import.removal.erasenow.erase.now = jetzt endgültig löschen

journal.window.import.consent.import.removal.found.detected.entities.drop.it.too.and = erkannte Entitäten lassen es auch los, und Stimmproben daraus verlassen deine bekannten Stimmen. die Entitäten, die du pflegst, bleiben, und was dein Journal über sie geschrieben hat, kann es noch erwähnen.

journal.window.import.consent.import.removal.found.detected.entities.drop.it.too.and.variant2 = erkannte Entitäten lassen es auch los, und Stimmproben daraus verlassen deine bekannten Stimmen. die Entitäten, die du pflegst, werden nicht endgültig gelöscht, und was dein Journal über sie geschrieben hat, kann es noch erwähnen.

journal.window.import.consent.import.removal.foundmany.detected.entities.drop.them.too.and = erkannte Entitäten lassen sie auch los, und Stimmproben aus ihnen verlassen deine bekannten Stimmen. die Entitäten, die du pflegst, bleiben, und was dein Journal über sie geschrieben hat, kann sie noch erwähnen.

journal.window.import.consent.import.removal.foundmany.detected.entities.drop.them.too.and.variant2 = erkannte Entitäten lassen sie auch los, und Stimmproben aus ihnen verlassen deine bekannten Stimmen. die Entitäten, die du pflegst, werden nicht endgültig gelöscht, und was dein Journal über sie geschrieben hat, kann sie noch erwähnen.

journal.window.import.consent.import.removal.lead.this.erases.the.import.s.own = dies löscht endgültig das Eigene des Imports:

journal.window.import.consent.import.removal.lead.this.moves.the.import.s.own = dies verschiebt das Eigene des Imports:

journal.window.import.consent.import.removal.movetotrash.move.to.trash = in den Papierkorb verschieben

journal.window.import.consent.import.removal.ownmany.this.erases.each.import.s.own = dies löscht endgültig je Import die eigenen Segmente, jede Originaldatei, die er behalten hat, und seine Zeile in deinem Importverlauf.

journal.window.import.consent.import.removal.ownmany.this.moves.each.import.s.own = dies verschiebt je Import die eigenen Segmente, jede Originaldatei, die er behalten hat, und seine Zeile in deinem Importverlauf.

journal.window.import.consent.import.removal.reach.it.leaves.your.journal.now.and = es verlässt dein Journal jetzt und kann nicht wiederhergestellt werden.

journal.window.import.consent.import.removal.reach.it.leaves.your.journal.now.and.variant2 = es verlässt dein Journal jetzt, wartet im Papierkorb und wird dann endgültig gelöscht. du kannst es bis dahin wiederherstellen.

journal.window.import.consent.import.removal.reachmany.they.leave.your.journal.now.and = sie verlassen dein Journal jetzt und können nicht wiederhergestellt werden.

journal.window.import.consent.import.removal.reachmany.they.leave.your.journal.now.and.variant2 = sie verlassen dein Journal jetzt, warten im Papierkorb und werden dann endgültig gelöscht. du kannst jedes einzelne bis dahin wiederherstellen.

journal.window.import.consent.import.removal.sharedset.some.of.what.they.brought.in = einiges, was sie hereingeholt haben, kann bleiben, weil ein früherer Import in deinem Journal es zuerst hereingeholt hat. es bleibt bei diesem Import.

journal.window.import.consent.import.removal.sharedset.some.of.what.they.brought.in.variant2 = einiges, was sie hereingeholt haben, kann bleiben und wird nicht endgültig gelöscht, weil ein früherer Import in deinem Journal es zuerst hereingeholt hat. es bleibt bei diesem Import.

journal.window.import.consent.import.removal.sharedunknown.some.of.what.it.brought.in = einiges, was es hereingeholt hat, kann bleiben, weil ein früherer Import in deinem Journal es zuerst hereingeholt hat. es bleibt bei diesem Import.

journal.window.import.consent.import.removal.sharedunknown.some.of.what.it.brought.in.variant2 = einiges, was es hereingeholt hat, kann bleiben und wird nicht endgültig gelöscht, weil ein früherer Import in deinem Journal es zuerst hereingeholt hat. es bleibt bei diesem Import.

journal.window.import.consent.import.removal.stays.activities.made.from.it.drop.it = Aktivitäten, die daraus erstellt wurden, lassen es jetzt los, und die Berichte seiner Tage und Wochen verlassen die Suche. dein Journal schreibt diese dann aus dem, was bleibt, neu, mit dem Denken, das du gewählt hast.

journal.window.import.consent.import.removal.staysbody.activities.summaries.and.entities.made.from = Aktivitäten, Zusammenfassungen und Entitäten, die anderswo in deinem Journal daraus erstellt wurden, werden nicht endgültig gelöscht und können es noch erwähnen.

journal.window.import.consent.import.removal.staysbody.activities.summaries.and.entities.made.from.variant2 = Aktivitäten, Zusammenfassungen und Entitäten, die anderswo in deinem Journal daraus erstellt wurden, bleiben, wo sie sind, und können es noch erwähnen.

journal.window.import.consent.import.removal.staysbodymany.activities.summaries.and.entities.made.from = Aktivitäten, Zusammenfassungen und Entitäten, die anderswo in deinem Journal aus ihnen erstellt wurden, werden nicht endgültig gelöscht und können sie noch erwähnen.

journal.window.import.consent.import.removal.staysbodymany.activities.summaries.and.entities.made.from.variant2 = Aktivitäten, Zusammenfassungen und Entitäten, die anderswo in deinem Journal aus ihnen erstellt wurden, bleiben, wo sie sind, und können sie noch erwähnen.

journal.window.import.consent.import.removal.staysmany.activities.made.from.them.drop.them = Aktivitäten, die aus ihnen erstellt wurden, lassen sie jetzt los, und die Berichte ihrer Tage und Wochen verlassen die Suche. dein Journal schreibt diese dann aus dem, was bleibt, neu, mit dem Denken, das du gewählt hast.

journal.window.import.consent.import.removal.title.delete.this.import = diesen Import löschen?

journal.window.import.consent.import.removal.title.erase.this.import.now = diesen Import jetzt endgültig löschen?

journal.window.import.consent.import.removal.tooknothing.everything.it.brought.in.stays.and = alles, was es hereingeholt hat, bleibt und wird nicht endgültig gelöscht, weil ein früherer Import in deinem Journal es zuerst hereingeholt hat. es bleibt bei diesem Import.

journal.window.import.consent.import.removal.tooknothing.everything.it.brought.in.stays.because = alles, was es hereingeholt hat, bleibt, weil ein früherer Import in deinem Journal es zuerst hereingeholt hat. es bleibt bei diesem Import.

journal.window.import.consent.removal.again.apple.health.erase = jeder Export aus Apple Health hält deinen ganzen Verlauf, also bringt ein erneuter Import davon zurück, was dies endgültig löscht.

journal.window.import.consent.removal.again.apple.health.move = jeder Export aus Apple Health hält deinen ganzen Verlauf, also bringt ein erneuter Import davon zurück, was dies löscht.

journal.window.import.consent.removal.again.oura.erase = ein Oura-Abgleich lässt aus, was dies endgültig löscht. einer, den du für die Tage anforderst, die er abdeckte, bringt es zurück.

journal.window.import.consent.removal.again.oura.move = ein Oura-Abgleich lässt aus, was dies löscht. einer, den du für die Tage anforderst, die er abdeckte, bringt es zurück.

journal.window.import.consent.removal.again.strava.erase = einen Strava-Download erneut zu importieren bringt die Trainings zurück, die dies endgültig löscht.

journal.window.import.consent.removal.again.strava.move = einen Strava-Download erneut zu importieren bringt die Trainings zurück, die dies löscht.

journal.window.import.consent.removal.multiple.erase = { $quantity ->
        [one] { $import_count } Import jetzt endgültig löschen?
       *[other] { $import_count } Importe jetzt endgültig löschen?
    }

journal.window.import.consent.removal.multiple.move = { $quantity ->
        [one] { $import_count } Import löschen?
       *[other] { $import_count } Importe löschen?
    }

journal.window.import.consent.removal.storage.total = Gesamtgröße: { $storage }

journal.window.import.content.fallback.activity.numbered = Aktivität { $number }

journal.window.import.content.fallback.audio = Audio-Import

journal.window.import.content.fallback.conversation.numbered = Gespräch { $number }

journal.window.import.content.fallback.conversation.segment = Gesprächssegment

journal.window.import.content.fallback.event.untitled = Termin ohne Titel

journal.window.import.content.fallback.image = Bildimport

journal.window.import.content.fallback.note = Notiz

journal.window.import.content.month.heading.count = { $quantity ->
        [one] { $month } { $year }, { $count } Element
       *[other] { $month } { $year }, { $count } Elemente
    }

journal.window.import.content.speaker.assistant = Assistent

journal.window.import.content.speaker.human = Mensch

journal.window.import.content.speaker.unknown = Unbekannt

journal.window.import.content.speaker.you = Du

journal.window.import.content.type.audio = Audio

journal.window.import.content.type.conversation = Gespräch

journal.window.import.content.type.document = Dokument

journal.window.import.content.type.event = Termin

journal.window.import.content.type.highlight.group = Hervorhebungen

journal.window.import.content.type.image = Bild

journal.window.import.content.type.note = Notiz

journal.window.import.content.type.pdf = pdf

journal.window.import.detail.content.attendees = { $quantity ->
        [one] { $count } teilnehmende Person
       *[other] { $count } teilnehmende Personen
    }

journal.window.import.detail.content.duration.minutes = { $minutes } min

journal.window.import.detail.content.highlights = { $quantity ->
        [one] { $count } Hervorhebung
       *[other] { $count } Hervorhebungen
    }

journal.window.import.detail.content.messages = { $quantity ->
        [one] { $count } Nachricht
       *[other] { $count } Nachrichten
    }

journal.window.import.detail.content.summary.count = { $quantity ->
        [one] { $source_name }, { $count } Element
       *[other] { $source_name }, { $count } Elemente
    }

journal.window.import.detail.count.entities = { $quantity ->
        [one] { $count } Entität
       *[other] { $count } Entitäten
    }

journal.window.import.detail.count.entries = { $quantity ->
        [one] { $count } Eintrag
       *[other] { $count } Einträge
    }

journal.window.import.detail.count.files = { $quantity ->
        [one] { $count } Datei
       *[other] { $count } Dateien
    }

journal.window.import.detail.duration.completed.day = { $quantity ->
        [one] abgeschlossen in { $count } Tag
       *[other] abgeschlossen in { $count } Tagen
    }

journal.window.import.detail.duration.completed.hour = { $quantity ->
        [one] abgeschlossen in { $count } Stunde
       *[other] abgeschlossen in { $count } Stunden
    }

journal.window.import.detail.duration.completed.minute = { $quantity ->
        [one] abgeschlossen in { $count } Minute
       *[other] abgeschlossen in { $count } Minuten
    }

journal.window.import.detail.duration.completed.short = abgeschlossen in unter einer Minute

journal.window.import.detail.duration.elapsed.day = { $quantity ->
        [one] { $count } Tag
       *[other] { $count } Tage
    }

journal.window.import.detail.duration.elapsed.hour = { $quantity ->
        [one] { $count } Stunde
       *[other] { $count } Stunden
    }

journal.window.import.detail.duration.elapsed.minute = { $quantity ->
        [one] { $count } Minute
       *[other] { $count } Minuten
    }

journal.window.import.detail.duration.elapsed.short = unter einer Minute

journal.window.import.detail.heading.timestamp = Import: { $date } · { $clock_time }

journal.window.import.detail.merge.body.days.hidden = { $quantity ->
        [one] 1 Tag mit Körperdaten war im Archiv verborgen, also ist er auch in diesem Journal verborgen.
       *[other] { $count } Tage mit Körperdaten waren im Archiv verborgen, also sind sie auch in diesem Journal verborgen.
    }

journal.window.import.detail.merge.clock.hours = { $quantity ->
        [one] 1 Stunde von { $source } kann zur falschen Zeit erscheinen: sie wurde in einer anderen Zeitzone gespeichert als in der, die dieses Journal für { $source } hält.
       *[other] { $count } Stunden von { $source } können zur falschen Zeit erscheinen: sie wurden in einer anderen Zeitzone gespeichert als in der, die dieses Journal für { $source } hält.
    }

journal.window.import.detail.merge.clock.under.hour = weniger als eine Stunde von { $source } kann zur falschen Zeit erscheinen: sie wurde in einer anderen Zeitzone gespeichert als in der, die dieses Journal für { $source } hält.

journal.window.import.detail.merge.copied = { $quantity ->
        [one] { $count } kopiert
       *[other] { $count } kopiert
    }

journal.window.import.detail.merge.created = { $quantity ->
        [one] { $count } erstellt
       *[other] { $count } erstellt
    }

journal.window.import.detail.merge.errored = { $quantity ->
        [one] { $count } mit Fehler
       *[other] { $count } mit Fehler
    }

journal.window.import.detail.merge.links.left.out = { $quantity ->
        [one] { $count } Entitätsverknüpfung ausgelassen
       *[other] { $count } Entitätsverknüpfungen ausgelassen
    }

journal.window.import.detail.merge.merged = { $quantity ->
        [one] { $count } zusammengeführt
       *[other] { $count } zusammengeführt
    }

journal.window.import.detail.merge.owner.collision = dieses Journal gehört { $journal_owner }, und das importierte Journal markiert { $archive_owner } als die Person, der es gehört. wem dieses Journal gehört, ändert sich nicht; die andere Person kam als normale Entität herein.

journal.window.import.detail.merge.owner.collision.staged = dieses Journal gehört { $journal_owner }, und das importierte Journal markiert { $archive_owner } als die Person, der es gehört. wem dieses Journal gehört, ändert sich nicht; die andere Person wurde vorgemerkt und nicht hinzugefügt.

journal.window.import.detail.merge.skipped = { $quantity ->
        [one] { $count } übersprungen
       *[other] { $count } übersprungen
    }

journal.window.import.detail.merge.staged = { $quantity ->
        [one] { $count } bereitgestellt
       *[other] { $count } bereitgestellt
    }

journal.window.import.detail.merge.staged.omitted = { $quantity ->
        [one] { $count } weitere bereitgestellt
       *[other] { $count } weitere bereitgestellt
    }

journal.window.import.detail.preview.other.units = { $count } { $unit }

journal.window.import.detail.preview.pages.unavailable = { $quantity ->
        [one] 1 Seite konnte nicht extrahiert werden
       *[other] { $count } Seiten konnten nicht extrahiert werden
    }

journal.window.import.detail.preview.recordings = { $recording_quantity ->
        [one] { $day_quantity ->
                [one] { $recordings } Aufnahme über { $days } Tag
               *[other] { $recordings } Aufnahme über { $days } Tage
            }
       *[other] { $day_quantity ->
                [one] { $recordings } Aufnahmen über { $days } Tag
               *[other] { $recordings } Aufnahmen über { $days } Tage
            }
    }

journal.window.import.detail.provenance.importer = { $source_type }-Importer

journal.window.import.detail.provenance.processed = verarbeitet { $finished_at }

journal.window.import.field.label.context = Kontext

journal.window.import.field.label.date = Datum

journal.window.import.field.label.keep.originals = Originale behalten

journal.window.import.field.label.manifest.filename = Manifest-Dateiname

journal.window.import.field.label.source = Quelle

journal.window.import.field.label.time.zone.in.recording.filenames = Zeitzone in den Dateinamen der Aufnahmen

journal.window.import.field.placeholder.e.g.paste.from.notes.app = z. B. aus der Notizen-App einfügen

journal.window.import.field.placeholder.e.g.rae.lunch.with.joe = z. B. rae Mittagessen mit joe

journal.window.import.field.placeholder.e.g.weekly.planning = z. B. Wochenplanung

journal.window.import.field.placeholder.paste.text.here = hier Text einfügen…

journal.window.import.field.placeholder.search.items = nach Elementen suchen…

journal.window.import.guide.apple.health.line1 = hol dir deinen { $slot1 }-Export

journal.window.import.guide.apple.health.line11 = bevor du importierst

journal.window.import.guide.apple.health.line13 = dieser Import liest Trainings, Schlaf, Achtsamkeitszeit, Herzfrequenz und andere Messwerte, tägliche Summen wie Schritte und Distanz sowie eingetragene Einträge wie Essen und Symptome. alles andere im Export ist ausgelassen, und die Zusammenfassung nennt die anderen Arten von Datensätzen, die er übersprungen hat. es wird in deinem Journalordner behalten, geht also mit, wohin dieser Ordner kopiert wird, und die verschlüsselte Sicherung schließt es ein, wenn du sie nutzt. der Export selbst wird nach dem Import nicht behalten.

journal.window.import.guide.apple.health.line15 = denselben Export erneut zu importieren bringt nur zurück, was du gelöscht hast, und ein neuerer Export fügt auch hinzu, was neu ist. ein Tag, den du auf der Seite Körper verborgen hast, bleibt verborgen.

journal.window.import.guide.apple.health.line3 = auf deinem iPhone öffne die { $slot1 }-App

journal.window.import.guide.apple.health.line4 = tippe auf { $slot1 }, dann dein Foto oder deine Initialen oben rechts

journal.window.import.guide.apple.health.line5 = tippe auf { $slot1 }, dann auf { $slot2 }. ein langer Verlauf kann eine Weile dauern, lass die App also offen, bis sie fertig ist

journal.window.import.guide.apple.health.line6 = sende { $slot1 } an diesen Computer. { $slot2 } an einen Mac, oder es in Dateien speichern und hierher kopieren, beides funktioniert

journal.window.import.guide.apple.health.line7 = lade es hier hoch

journal.window.import.guide.apple.health.line9 = wenn der Export fast sofort fertig ist und die Datei winzig ist, hat es nicht geklappt. exportiere ihn erneut.

journal.window.import.guide.chatgpt.line1 = exportiere deinen { $slot1 }-Verlauf

journal.window.import.guide.chatgpt.line10 = lade die { $slot1 }-Datei hier hoch

journal.window.import.guide.chatgpt.line3 = öffne [{ $slot1 }]({ $slot2 })

journal.window.import.guide.chatgpt.line4 = klicke auf dein Profilsymbol in der unteren linken Ecke

journal.window.import.guide.chatgpt.line5 = wähle { $slot1 }

journal.window.import.guide.chatgpt.line6 = geh zu { $slot1 }

journal.window.import.guide.chatgpt.line7 = klicke auf { $slot1 }

journal.window.import.guide.chatgpt.line8 = du erhältst eine E-Mail mit einem Link zum Herunterladen. das kann ein paar Minuten dauern

journal.window.import.guide.chatgpt.line9 = lade die { $slot1 }-Datei aus der E-Mail herunter

journal.window.import.guide.claude.line1 = exportiere deinen { $slot1 }-Verlauf

journal.window.import.guide.claude.line10 = lade die { $slot1 }-Datei hier hoch

journal.window.import.guide.claude.line3 = öffne [{ $slot1 }]({ $slot2 })

journal.window.import.guide.claude.line4 = klicke auf dein Profilsymbol in der unteren linken Ecke

journal.window.import.guide.claude.line5 = wähle { $slot1 }

journal.window.import.guide.claude.line6 = wähle { $slot1 } in der Seitenleiste, dann scrolle zum Abschnitt { $slot2 }

journal.window.import.guide.claude.line7 = klicke auf { $slot1 }

journal.window.import.guide.claude.line8 = du erhältst eine E-Mail mit einem Link zum Herunterladen

journal.window.import.guide.claude.line9 = lade die { $slot1 }-Datei aus der E-Mail herunter

journal.window.import.guide.gemini.line1 = exportiere deinen { $slot1 }-Verlauf

journal.window.import.guide.gemini.line10 = lade die { $slot1 }-Datei herunter, wenn sie bereit ist

journal.window.import.guide.gemini.line11 = lade die { $slot1 }-Datei hier hoch

journal.window.import.guide.gemini.line3 = geh zu [{ $slot1 }]({ $slot2 })

journal.window.import.guide.gemini.line4 = klicke auf { $slot1 }

journal.window.import.guide.gemini.line5 = scrolle nach unten und wähle nur { $slot1 }

journal.window.import.guide.gemini.line6 = klicke auf { $slot1 }, dann wähle alles ab außer { $slot2 }

journal.window.import.guide.gemini.line7 = klicke auf { $slot1 }, dann scrolle nach unten und klicke auf { $slot2 }

journal.window.import.guide.gemini.line8 = wähle { $slot1 }, Dateityp { $slot2 } und deine bevorzugte Größe

journal.window.import.guide.gemini.line9 = klicke auf { $slot1 }. das kann je nach deinem Verlauf Minuten bis Stunden dauern

journal.window.import.guide.ics.line1 = { $slot1 }

journal.window.import.guide.ics.line10 = wähle in der Seitenleiste den Kalender, den du exportieren willst

journal.window.import.guide.ics.line11 = geh zu { $slot1 }

journal.window.import.guide.ics.line12 = speichere die { $slot1 }-Datei

journal.window.import.guide.ics.line13 = lade es hier hoch

journal.window.import.guide.ics.line15 = { $slot1 }

journal.window.import.guide.ics.line17 = öffne [{ $slot1 }-Kalender]({ $slot2 })

journal.window.import.guide.ics.line18 = geh zu { $slot1 }

journal.window.import.guide.ics.line19 = wähle { $slot1 }

journal.window.import.guide.ics.line20 = unter { $slot1 } wähle den Kalender und wähle { $slot2 }

journal.window.import.guide.ics.line21 = klicke auf den { $slot1 }-Link, um herunterzuladen

journal.window.import.guide.ics.line22 = lade die { $slot1 }-Datei hier hoch

journal.window.import.guide.ics.line3 = öffne [{ $slot1 }-Einstellungen]({ $slot2 })

journal.window.import.guide.ics.line4 = klicke auf { $slot1 }. das lädt eine { $slot2 }-Datei herunter, die { $slot3 }-Dateien für jeden Kalender enthält

journal.window.import.guide.ics.line5 = lade die { $slot1 }-Datei hier hoch

journal.window.import.guide.ics.line7 = { $slot1 }

journal.window.import.guide.ics.line9 = öffne die { $slot1 }-App auf deinem Mac

journal.window.import.guide.journal.archive.line1 = ein Journal verschieben

journal.window.import.guide.journal.archive.line3 = auf dem Journal, das du verschieben willst, öffne seinen Importbildschirm.

journal.window.import.guide.journal.archive.line4 = wähle { $slot1 } und wähle { $slot2 }.

journal.window.import.guide.journal.archive.line5 = bring die heruntergeladene { $slot1 } zu diesem Journal.

journal.window.import.guide.journal.archive.line6 = lade die { $slot1 } hier hoch, wähle { $slot2 }, um die Tage darin zu prüfen, und wähle dann { $slot3 }, um es zusammenzuführen.

journal.window.import.guide.journal.archive.line8 = wenn das andere Journal auf einem anderen Rechner ist, lade seine { $slot1 } zuerst von dessen Importbildschirm herunter, dann lade sie hier hoch.

journal.window.import.guide.step.number = Schritt { $step_number }

journal.window.import.guide.strava.line1 = hol dir deinen { $slot1 }-Download

journal.window.import.guide.strava.line10 = der Download kann groß sein, denn er hält deine Fotos und mehr. dieser Import öffnet darin nur die Liste deiner Trainings, { $slot1 }. wenn du den Rest lieber nicht hochladen willst, entpacke ihn und lade nur diese Datei hoch.

journal.window.import.guide.strava.line12 = bevor du importierst

journal.window.import.guide.strava.line14 = diese Trainings werden in deinem Journalordner behalten, gehen also mit, wohin dieser Ordner kopiert wird, und wenn du die verschlüsselte Sicherung nutzt, schließt sie die Trainings ein. dein { $slot1 }-Download selbst wird nach dem Import nicht behalten.

journal.window.import.guide.strava.line3 = melde dich bei strava.com auf einem Computer an. { $slot1 } bietet den Download nur auf seiner Website an, nicht in seiner App

journal.window.import.guide.strava.line4 = fahre oben rechts über deinen Namen und wähle { $slot1 }

journal.window.import.guide.strava.line5 = wähle { $slot1 }, dann { $slot2 } unter { $slot3 }

journal.window.import.guide.strava.line6 = wähle { $slot1 }

journal.window.import.guide.strava.line7 = { $slot1 } schickt dir per E-Mail einen Link. das kann ein paar Stunden dauern. lade die { $slot2 }-Datei herunter

journal.window.import.guide.strava.line8 = lade es hier hoch

journal.window.import.handoff.busy = die App fügt noch Dateien hinzu. versuch es gleich noch einmal.

journal.window.import.handoff.cancelled = keine Dateien gewählt

journal.window.import.handoff.date = wähle ein Datum, oder lass es leer, damit dein Journal wählt.

journal.window.import.handoff.delivered = { $name } ist in deinem Journal angekommen

journal.window.import.handoff.device = von { $device }

journal.window.import.handoff.health.expiry = prüfe diesen Export innerhalb einer Stunde, bevor dein Journal die wartende Kopie entfernt.

journal.window.import.handoff.health.review = Apple Health-Import prüfen

journal.window.import.handoff.health.unavailable = das Öffnen dieses Exports ist nicht gelungen. wähle ihn erneut, um ihn zu prüfen.

journal.window.import.handoff.notebook = Notizbuchname (optional)

journal.window.import.handoff.origin.android = von der solstone-App für Android

journal.window.import.handoff.origin.browser = von deinem Browser

journal.window.import.handoff.origin.cli = von der Befehlszeile

journal.window.import.handoff.origin.desktop = von einem Computer

journal.window.import.handoff.origin.ios = von der solstone-App für iPhone und iPad

journal.window.import.handoff.origin.linux = von der solstone-App für Linux

journal.window.import.handoff.origin.macos = von der solstone-App für den Mac

journal.window.import.handoff.origin.mobile = von einem mobilen Gerät

journal.window.import.handoff.origin.windows = von der solstone-App für Windows

journal.window.import.handoff.refused = die App konnte diese Datei nicht hinzufügen. versuch es noch einmal.

journal.window.import.handoff.sending = sendet { $name }

journal.window.import.handoff.waiting = wartet darauf, { $name } zu senden

journal.window.import.history.archive.left.out = { $quantity ->
        [one] { $count } Mitglied ausgelassen
       *[other] { $count } Mitglieder ausgelassen
    }

journal.window.import.history.archive.processing = { $quantity ->
        [one] { $count } wartet auf die Verarbeitung
       *[other] { $count } warten auf die Verarbeitung
    }

journal.window.import.history.archive.rejected = { $quantity ->
        [one] { $count } Teil abgelehnt
       *[other] { $count } Teile abgelehnt
    }

journal.window.import.history.archive.remaining = { $quantity ->
        [one] { $count } noch zu speichern
       *[other] { $count } noch zu speichern
    }

journal.window.import.history.archive.saved = { $quantity ->
        [one] { $count } Teil gespeichert
       *[other] { $count } Teile gespeichert
    }

journal.window.import.history.archive.unavailable = { $quantity ->
        [one] { $count } nicht verfügbar
       *[other] { $count } nicht verfügbar
    }

journal.window.import.history.count.entries.unknown = { $import_quantity ->
        [one] { $imports } Import, —.
       *[other] { $imports } Importe, —.
    }

journal.window.import.history.count.summary = { $import_quantity ->
        [one] { $entry_quantity ->
            [one] { $imports } Import, { $entries } Eintrag.
           *[other] { $imports } Import, { $entries } Einträge.
        }
       *[other] { $entry_quantity ->
            [one] { $imports } Importe, { $entries } Eintrag.
           *[other] { $imports } Importe, { $entries } Einträge.
        }
    }

journal.window.import.history.load.more.count = { $total_quantity ->
        [one] { $shown_quantity ->
            [one] mehr laden ({ $shown_count } von { $total_count } angezeigt)
           *[other] mehr laden ({ $shown_count } von { $total_count } angezeigt)
        }
       *[other] { $shown_quantity ->
            [one] mehr laden ({ $shown_count } von { $total_count } angezeigt)
           *[other] mehr laden ({ $shown_count } von { $total_count } angezeigt)
        }
    }

journal.window.import.history.stalled.import = stockt: seit 10 Minuten keine Aktualisierungen. Import-ID: { $import_id }.

journal.window.import.import.delete.copy.back.back = zurück

journal.window.import.import.delete.copy.bodyunknown.the.body.data.it.brought.in = die Körperdaten, die es hereingeholt hat

journal.window.import.import.delete.copy.busy.this.is.already.being.moved.or = das wird bereits verschoben oder endgültig gelöscht. lade in einer Minute neu, um zu prüfen.

journal.window.import.import.delete.copy.cancel.cancel = abbrechen

journal.window.import.import.delete.copy.delete.delete = löschen

journal.window.import.import.delete.copy.done.done = fertig

journal.window.import.import.delete.copy.eraseimporting.this.wasn.t.erased.because.it = das wurde nicht endgültig gelöscht, weil es noch importiert wird. versuch es noch einmal, wenn es fertig ist.

journal.window.import.import.delete.copy.eraseother.this.wasn.t.erased.try.again = das wurde nicht endgültig gelöscht. versuch es noch einmal.

journal.window.import.import.delete.copy.hideday.a.day.you.hide.on.the = ein Tag, den du auf der Seite Körper verbirgst, bleibt beim erneuten Import oder Abgleich verborgen.

journal.window.import.import.delete.copy.importing.this.wasn.t.moved.because.it = das wurde nicht verschoben, weil es noch importiert wird. versuch es noch einmal, wenn es fertig ist.

journal.window.import.import.delete.copy.message.it.may.already.be.back.in = es ist vielleicht schon wieder in deinem Journal. lade neu, um zu prüfen.

journal.window.import.import.delete.copy.message.it.may.already.be.erased.reload = es ist vielleicht schon endgültig gelöscht. lade neu, um zu prüfen.

journal.window.import.import.delete.copy.message.it.may.already.be.in.the = es ist vielleicht schon im Papierkorb. lade neu, um zu prüfen.

journal.window.import.import.delete.copy.message.it.s.back.in.your.journal = es ist wieder in deinem Journal.

journal.window.import.import.delete.copy.message.it.s.in.the.trash.you = es ist im Papierkorb. du kannst es von dort noch einmal versuchen.

journal.window.import.import.delete.copy.message.it.s.still.in.the.trash = es ist noch im Papierkorb. du kannst es von dort noch einmal versuchen.

journal.window.import.import.delete.copy.message.it.s.still.in.your.journal = es ist noch in deinem Journal.

journal.window.import.import.delete.copy.message.they.may.already.be.back.in = sie sind vielleicht schon wieder in deinem Journal. lade neu, um zu prüfen.

journal.window.import.import.delete.copy.message.they.may.already.be.erased.reload = sie sind vielleicht schon endgültig gelöscht. lade neu, um zu prüfen.

journal.window.import.import.delete.copy.message.they.may.already.be.in.the = sie sind vielleicht schon im Papierkorb. lade neu, um zu prüfen.

journal.window.import.import.delete.copy.message.they.re.back.in.your.journal = sie sind wieder in deinem Journal.

journal.window.import.import.delete.copy.message.they.re.still.in.the.trash = sie sind noch im Papierkorb. du kannst sie von dort noch einmal versuchen.

journal.window.import.import.delete.copy.oneunknown.any.of.its.segments.waiting.in = alle seine Segmente, die für sich im Papierkorb warten, werden mit ihm endgültig gelöscht.

journal.window.import.import.delete.copy.opentrash.open.trash = Papierkorb öffnen

journal.window.import.import.delete.copy.originalunknown.any.original.file.it.kept = jede Originaldatei, die es behalten hat

journal.window.import.import.delete.copy.other.this.wasn.t.moved.try.again = das wurde nicht verschoben. versuch es noch einmal.

journal.window.import.import.delete.copy.recover.recover = wiederherstellen

journal.window.import.import.delete.copy.row.its.row.in.your.import.history = seine Zeile in deinem Importverlauf

journal.window.import.import.delete.copy.segmentsunknown.the.segments.it.brought.in = die Segmente, die es hereingeholt hat

journal.window.import.import.delete.copy.select.select = auswählen

journal.window.import.import.delete.copy.selectall.select.all = alle auswählen

journal.window.import.import.delete.copy.title.couldn.t.erase.those = die konnten nicht endgültig gelöscht werden

journal.window.import.import.delete.copy.title.couldn.t.move.that.to.the = das konnte nicht in den Papierkorb verschoben werden

journal.window.import.import.delete.copy.title.couldn.t.move.those.to.the = die konnten nicht in den Papierkorb verschoben werden

journal.window.import.import.delete.copy.title.couldn.t.recover.those = die konnten nicht wiederhergestellt werden

journal.window.import.import.delete.copy.title.moved.to.the.trash = in den Papierkorb verschoben

journal.window.import.import.delete.copy.title.your.journal.didn.t.answer = dein Journal hat nicht geantwortet

journal.window.import.import.delete.copy.workoutsunknown.the.workouts.it.brought.in = die Trainings, die es hereingeholt hat

journal.window.import.import.source.names.audio.archive.audio.archive = Audio-Archiv

journal.window.import.import.source.names.audio.audio.folder = Audio-Ordner

journal.window.import.import.source.names.document.document = Dokument

journal.window.import.import.source.names.ics.calendar = Kalender

journal.window.import.import.source.names.image.image = Bild

journal.window.import.import.source.names.journal.archive.journal = Journal

journal.window.import.import.source.names.quick.quick.import = Schnellimport

journal.window.import.import.source.names.recording.meeting.audio = Besprechungs-Audio

journal.window.import.import.source.names.text.text = Text

journal.window.import.import.view.choose.the.time.zone.in.the = wähle die Zeitzone in den Dateinamen der Aufnahmen, oder gib den Manifest-Dateinamen ein.

journal.window.import.import.view.could.not.read.the.archive.size = die Archivgröße ließ sich nicht lesen

journal.window.import.import.view.could.not.read.the.import.size = die Importgröße ließ sich nicht lesen

journal.window.import.import.view.import.failed = Import ist nicht gelungen

journal.window.import.import.view.import.status.unavailable = Importstatus nicht verfügbar

journal.window.import.import.view.import.unconfirmed = Import unbestätigt

journal.window.import.import.view.nothing.new = nichts Neues

journal.window.import.import.view.path.not.found = Pfad nicht gefunden

journal.window.import.import.view.please.enter.the.path.to.your = Bitte gib den Pfad zu deinem Ordner ein.

journal.window.import.import.view.please.select.a.file.to.import = Bitte wähle eine Datei zum Importieren aus.

journal.window.import.import.view.recordings.saved = Aufnahmen gespeichert

journal.window.import.import.view.recordings.saved.processing.continues = Aufnahmen gespeichert; Verarbeitung läuft weiter

journal.window.import.import.view.validation.failed = Validierung ist nicht gelungen

journal.window.import.label.source.obsidian = { $slot1 }

journal.window.import.label.source.oura = { $slot1 }

journal.window.import.label.source.plaud = { $slot1 }

journal.window.import.link.label.back.to.imports = ← zurück zu Importen

journal.window.import.link.label.back.to.sources = ← zurück zu Quellen

journal.window.import.link.label.browse.what.was.imported = durchsehen, was importiert wurde →

journal.window.import.link.label.choose.a.source = eine Quelle wählen →

journal.window.import.link.label.download.this.journal = dieses Journal herunterladen

journal.window.import.link.label.import.another.source = eine weitere Quelle importieren

journal.window.import.link.label.see.it.in.the.body.app = in der Körper-App ansehen →

journal.window.import.link.label.see.when.your.devices.last.added.something = sehen, wann deine Geräte zuletzt etwas hinzufügten →

journal.window.import.link.label.view.imported.content = importierten Inhalt ansehen

journal.window.import.option.label.all.months = alle Monate

journal.window.import.option.label.all.sources = alle Quellen

journal.window.import.option.label.choose.a.time.zone = eine Zeitzone wählen

journal.window.import.owner.desc.import.from.any.source.above = importiere aus einer beliebigen Quelle oben.

journal.window.import.owner.heading.couldn.t.load.your.import.history = dein Importverlauf ließ sich nicht laden

journal.window.import.owner.heading.no.imports.match.this.filter = keine Importe passen zu diesem Filter

journal.window.import.owner.heading.no.imports.yet = noch keine Importe

journal.window.import.owner.message.check.your.import.history.before.trying = prüfe deinen Importverlauf, bevor du es noch einmal versuchst.

journal.window.import.owner.title.upload.interrupted = Hochladen unterbrochen

journal.window.import.ox.detail.back.to.imports = zurück zu Importen

journal.window.import.ox.detail.checking = wird geprüft, was in { $name } ist…

journal.window.import.ox.detail.context.hint = eine kurze Notiz, worum es geht, damit du es später leichter findest

journal.window.import.ox.detail.context.label = Kontext (optional)

journal.window.import.ox.detail.discard.body = das entfernt die noch nicht gestartete Datei aus deinem Journal. noch ist nichts importiert.

journal.window.import.ox.detail.discard.button = wartende Kopie verwerfen

journal.window.import.ox.detail.discard.cancel = abbrechen

journal.window.import.ox.detail.discard.confirm = verwerfen

journal.window.import.ox.detail.discard.title = diese wartende Kopie verwerfen?

journal.window.import.ox.detail.expired.badge = abgelaufen

journal.window.import.ox.detail.expired.body = diese wartende Kopie wurde nach einer Stunde entfernt, ohne dass sie gestartet wurde.

journal.window.import.ox.detail.expired.recovery = wenn du es aus einer solstone-App hinzugefügt hast, öffne dort den Import, um es erneut zu senden, oder geh zurück zu den Importen und wähle die Datei erneut.

journal.window.import.ox.detail.expiry.note = wenn du ohne zu starten gehst, entfernt dein Journal die wartende Kopie nach einer Stunde.

journal.window.import.ox.detail.facet.label = in welche Facette sollen diese gehen?

journal.window.import.ox.detail.found.heading = was dein Journal gefunden hat

journal.window.import.ox.detail.keep.originals.hint = schalte das ein, wenn du eine Kopie der Originaldateien in deinem Journal willst.

journal.window.import.ox.detail.no.facet = keine Facette

journal.window.import.ox.detail.nothing.new = nichts Neues aus dieser Datei hereinzuholen.

journal.window.import.ox.detail.space.available = braucht etwa { $need } · { $free } frei, wo dein Journal liegt

journal.window.import.ox.detail.space.insufficient = nicht genug freier Speicher, wo dein Journal liegt, um das hereinzuholen. mach dort Speicher frei und versuch es noch einmal.

journal.window.import.ox.detail.try.preview.again = erneut die Vorschau versuchen

journal.window.import.ox.discard.already.started = der Import ist schon gestartet oder verarbeitet

journal.window.import.ox.discard.import.active = der Import ist derzeit aktiv

journal.window.import.ox.discard.import.locked = der Import ist gesperrt

journal.window.import.ox.discard.journal.busy = Journal ist beschäftigt

journal.window.import.ox.discard.metadata.unverified = Import-Metadaten lassen sich nicht überprüfen

journal.window.import.ox.discard.not.waiting = der Import wartet nicht mehr auf den Start

journal.window.import.ox.history.review.link = prüfen und bestätigen

journal.window.import.ox.status.ready.to.review = bereit zum Prüfen

journal.window.import.ox.tray.attention.badge = braucht Aufmerksamkeit

journal.window.import.ox.tray.attention.remove = öffne den Import in der solstone-App, um ihn zu entfernen.

journal.window.import.ox.tray.attention.retryable = öffne den Import in der solstone-App, um es noch einmal zu versuchen oder zu entfernen.

journal.window.import.ox.tray.completed.summary = abgeschlossen · { $summary }

journal.window.import.ox.tray.heading = von diesem Gerät

journal.window.import.ox.tray.in.journal = in deinem Journal

journal.window.import.ox.tray.in.journal.stage = in deinem Journal · { $stage }

journal.window.import.ox.tray.review.action = prüfen und bestätigen

journal.window.import.ox.tray.review.detail = in deinem Journal gespeichert. Vorschau anzeigen und bestätigen, um zu starten.

journal.window.import.ox.tray.review.heading = wartet auf deine Prüfung

journal.window.import.ox.tray.sending.device = sendet von deinem { $device }…

journal.window.import.ox.tray.sending.generic = sendet an dein Journal…

journal.window.import.ox.tray.view.import = Import ansehen

journal.window.import.ox.tray.waiting = wartet in der solstone-App

journal.window.import.preview.apple.daily.totals = { $quantity ->
        [one] { $count } Tagessumme
       *[other] { $count } Tagessummen
    }

journal.window.import.preview.apple.daily.totals.label = { $quantity ->
        [one] Tagessumme
       *[other] Tagessummen
    }

journal.window.import.preview.apple.left.out = { $quantity ->
        [one] { $count } andere Art ausgelassen, in der Zusammenfassung genannt
       *[other] { $count } andere Arten ausgelassen, in der Zusammenfassung genannt
    }

journal.window.import.preview.apple.logged = { $quantity ->
        [one] { $count } protokollierter Eintrag
       *[other] { $count } protokollierte Einträge
    }

journal.window.import.preview.apple.logged.label = { $quantity ->
        [one] protokollierter Eintrag
       *[other] protokollierte Einträge
    }

journal.window.import.preview.apple.mindful = { $quantity ->
        [one] { $count } Achtsamkeitssitzung
       *[other] { $count } Achtsamkeitssitzungen
    }

journal.window.import.preview.apple.mindful.label = { $quantity ->
        [one] Achtsamkeitssitzung
       *[other] Achtsamkeitssitzungen
    }

journal.window.import.preview.apple.move.listed = { $quantity ->
        [one] ja sagen kann bis zu { $count } Datensatz, der schon auf der Seite Aktivitäten aufgeführt ist, in die Facette dieses Imports verschieben
       *[other] ja sagen kann bis zu { $count } Datensätze, die schon auf der Seite Aktivitäten aufgeführt sind, in die Facette dieses Imports verschieben
    }

journal.window.import.preview.apple.nights = { $quantity ->
        [one] { $count } Nacht aus Schlafphasen zusammengesetzt
       *[other] { $count } Nächte aus Schlafphasen zusammengesetzt
    }

journal.window.import.preview.apple.nights.label = { $quantity ->
        [one] Nacht aus Schlafphasen zusammengesetzt
       *[other] Nächte aus Schlafphasen zusammengesetzt
    }

journal.window.import.preview.apple.readings = { $quantity ->
        [one] { $count } Messwert
       *[other] { $count } Messwerte
    }

journal.window.import.preview.apple.readings.label = { $quantity ->
        [one] Messwert
       *[other] Messwerte
    }

journal.window.import.preview.apple.workouts = { $quantity ->
        [one] { $count } Training
       *[other] { $count } Trainings
    }

journal.window.import.preview.apple.workouts.label = { $quantity ->
        [one] Training
       *[other] Trainings
    }

journal.window.import.preview.archive.days = { $quantity ->
        [one] { $count } Tag
       *[other] { $count } Tage
    }

journal.window.import.preview.archive.left.out = { $quantity ->
        [one] { $count } ausgelassen
       *[other] { $count } ausgelassen
    }

journal.window.import.preview.archive.recording.bytes = { $quantity ->
        [one] { $count } Aufnahme-Byte
       *[other] { $count } Aufnahme-Bytes
    }

journal.window.import.preview.archive.saved = { $quantity ->
        [one] { $count } Audio-Teil schon gespeichert
       *[other] { $count } Audio-Teile schon gespeichert
    }

journal.window.import.preview.archive.source.files = { $quantity ->
        [one] { $count } Ursprungsdatei
       *[other] { $count } Ursprungsdateien
    }

journal.window.import.preview.date.range = { $first_date } bis { $last_date }

journal.window.import.preview.skipped = { $quantity ->
        [one] { $count } übersprungen
       *[other] { $count } übersprungen
    }

journal.window.import.preview.storage.available = braucht etwa { $needed } frei, und { $free } ist frei

journal.window.import.preview.workouts.move.listed = { $quantity ->
        [one] { $count } Training, das schon auf der Seite Aktivitäten aufgeführt ist, würde von { $sources } in die Facette dieses Imports wechseln
       *[other] { $count } Trainings, die schon auf der Seite Aktivitäten aufgeführt sind, würden von { $sources } in die Facette dieses Imports wechseln
    }

journal.window.import.progress.archive.left.out = { $quantity ->
        [one] { $count } ausgelassen
       *[other] { $count } ausgelassen
    }

journal.window.import.progress.archive.processed = { $quantity ->
        [one] { $count } verarbeitet
       *[other] { $count } verarbeitet
    }

journal.window.import.progress.archive.processing = { $quantity ->
        [one] { $count } wartet auf die Verarbeitung
       *[other] { $count } warten auf die Verarbeitung
    }

journal.window.import.progress.archive.remaining = { $quantity ->
        [one] { $count } noch zu speichern
       *[other] { $count } noch zu speichern
    }

journal.window.import.progress.archive.saved = { $quantity ->
        [one] { $count } Audio-Teil gespeichert
       *[other] { $count } Audio-Teile gespeichert
    }

journal.window.import.progress.body.days = Tag { $days_done } von { $days_total }

journal.window.import.progress.completed.pages = { $quantity ->
        [one] teilweise extrahiert (1 Seite nicht verfügbar)
       *[other] teilweise extrahiert ({ $count } Seiten nicht verfügbar)
    }

journal.window.import.progress.completed.time = dauerte { $elapsed_time }

journal.window.import.progress.count.entities = { $quantity ->
        [one] { $count } Entität
       *[other] { $count } Entitäten
    }

journal.window.import.progress.count.entities.found = { $quantity ->
        [one] { $count } Entität gefunden
       *[other] { $count } Entitäten gefunden
    }

journal.window.import.progress.count.entries = { $quantity ->
        [one] { $count } Eintrag
       *[other] { $count } Einträge
    }

journal.window.import.progress.date.end = bis { $last_date }

journal.window.import.progress.date.start = von { $first_date }

journal.window.import.progress.items.ratio = { $total_quantity ->
        [one] { $processed }/{ $total } Element
       *[other] { $processed }/{ $total } Elemente
    }

journal.window.import.progress.reading.source = liest { $source }…

journal.window.import.progress.repeat.document = ein weiteres Dokument importieren

journal.window.import.progress.repeat.image = ein weiteres Bild importieren

journal.window.import.progress.stage.saving = Wird gespeichert…

journal.window.import.progress.thinking.held = { $quantity ->
        [one] { $count } zurückgehalten — { $health_link_start }auf Gesundheit prüfen{ $health_link_end }
       *[other] { $count } zurückgehalten — { $health_link_start }auf Gesundheit prüfen{ $health_link_end }
    }

journal.window.import.progress.thinking.ready = { $total_quantity ->
        [one] { $ready } von { $total } Aufnahme bereit zum Finden
       *[other] { $ready } von { $total } Aufnahmen bereit zum Finden
    }

journal.window.import.progress.thinking.unreadable = { $quantity ->
        [one] { $count } konnte nicht geprüft werden
       *[other] { $count } konnten nicht geprüft werden
    }

journal.window.import.progress.thinking.waiting = { $quantity ->
        [one] { $count } wartet
       *[other] { $count } warten
    }

journal.window.import.progress.time.elapsed = { $elapsed_time } vergangen

journal.window.import.progress.time.hours.minutes = { $hours } h { $minutes } min

journal.window.import.progress.time.minutes.seconds = { $minutes } min { $seconds } s

journal.window.import.progress.time.seconds = { $seconds } s

journal.window.import.progress.time.stage.total = { $stage_time } ({ $total_time } gesamt)

journal.window.import.removal.date.imported = importiert am { $day }

journal.window.import.removal.items.body.day = Körperdaten von { $day }

journal.window.import.removal.items.body.span = Körperdaten von { $first_day } bis { $last_day }

journal.window.import.removal.items.originals = { $quantity ->
        [one] die Originaldatei, die er behalten hat
       *[other] die { $original_count } Originaldateien, die er behalten hat
    }

journal.window.import.removal.items.segment.day = { $quantity ->
        [one] { $segment_count } Segment von { $day }
       *[other] { $segment_count } Segmente von { $day }
    }

journal.window.import.removal.items.segment.span = { $quantity ->
        [one] { $segment_count } Segment von { $first_day } bis { $last_day }
       *[other] { $segment_count } Segmente von { $first_day } bis { $last_day }
    }

journal.window.import.removal.items.workout.day = { $quantity ->
        [one] { $workout_count } Training von { $day }
       *[other] { $workout_count } Trainings von { $day }
    }

journal.window.import.removal.items.workout.span = { $quantity ->
        [one] { $workout_count } Training von { $first_day } bis { $last_day }
       *[other] { $workout_count } Trainings von { $first_day } bis { $last_day }
    }

journal.window.import.removal.receipt.erase.date.many = sie werden am { $scheduled_erase_day } endgültig gelöscht, wenn du sie nicht wiederherstellst.

journal.window.import.removal.receipt.erase.date.one = wird am { $scheduled_erase_day } endgültig gelöscht, wenn du ihn nicht wiederherstellst.

journal.window.import.removal.receipt.erased.import = Import aus deinem Journal endgültig gelöscht

journal.window.import.removal.receipt.erased.imports = { $quantity ->
        [one] { $import_count } Import aus deinem Journal endgültig gelöscht
       *[other] { $import_count } Importe aus deinem Journal endgültig gelöscht
    }

journal.window.import.removal.receipt.gone.more = { $quantity ->
        [one] { $no_longer_listed } mehr ist nicht mehr aufgeführt.
       *[other] { $no_longer_listed } mehr sind nicht mehr aufgeführt.
    }

journal.window.import.removal.receipt.import.erase.failed = diesen Import endgültig zu löschen ist nicht gelungen

journal.window.import.removal.receipt.import.recover.failed = diesen Import wiederherzustellen ist nicht gelungen

journal.window.import.removal.receipt.marked = { $quantity ->
        [one] { $marked_count } ließ sich nicht verschieben und ist unten markiert.
       *[other] { $marked_count } ließen sich nicht verschieben und sind unten markiert.
    }

journal.window.import.removal.receipt.moved.imports = { $quantity ->
        [one] { $import_count } Import in den Papierkorb verschoben
       *[other] { $import_count } Importe in den Papierkorb verschoben
    }

journal.window.import.removal.receipt.none.erased.note = { $quantity ->
        [one] such ihn unten oder im Papierkorb.
       *[other] such sie unten oder im Papierkorb.
    }

journal.window.import.removal.receipt.none.gone = { $quantity ->
        [one] er ist hier nicht mehr aufgeführt.
       *[other] sie sind hier nicht mehr aufgeführt.
    }

journal.window.import.removal.receipt.none.still = { $quantity ->
        [one] { $still_listed } ist noch in deinem Journal, unten markiert.
       *[other] { $still_listed } sind noch in deinem Journal, unten markiert.
    }

journal.window.import.removal.receipt.others.gone = { $quantity ->
        [one] der andere ist hier nicht mehr aufgeführt.
       *[other] die anderen sind hier nicht mehr aufgeführt.
    }

journal.window.import.removal.receipt.part.erased.note = { $quantity ->
        [one] der andere wurde nicht endgültig gelöscht. such ihn unten oder im Papierkorb.
       *[other] die anderen wurden nicht endgültig gelöscht. such sie unten oder im Papierkorb.
    }

journal.window.import.removal.receipt.part.recovered.note = { $quantity ->
        [one] der andere ist noch im Papierkorb. von dort kannst du es noch einmal versuchen.
       *[other] die anderen sind noch im Papierkorb. von dort kannst du es noch einmal versuchen.
    }

journal.window.import.removal.receipt.partial.erase = { $quantity ->
        [one] { $completed_quantity ->
        [one] { $completed_count } von { $import_count } endgültig gelöscht
       *[other] { $completed_count } von { $import_count } endgültig gelöscht
    }
       *[other] { $completed_quantity ->
        [one] { $completed_count } von { $import_count } endgültig gelöscht
       *[other] { $completed_count } von { $import_count } endgültig gelöscht
    }
    }

journal.window.import.removal.receipt.partial.move = { $quantity ->
        [one] { $completed_quantity ->
        [one] { $completed_count } von { $import_count } in den Papierkorb verschoben
       *[other] { $completed_count } von { $import_count } in den Papierkorb verschoben
    }
       *[other] { $completed_quantity ->
        [one] { $completed_count } von { $import_count } in den Papierkorb verschoben
       *[other] { $completed_count } von { $import_count } in den Papierkorb verschoben
    }
    }

journal.window.import.removal.receipt.partial.recover = { $quantity ->
        [one] { $completed_quantity ->
        [one] { $completed_count } von { $import_count } wiederhergestellt
       *[other] { $completed_count } von { $import_count } wiederhergestellt
    }
       *[other] { $completed_quantity ->
        [one] { $completed_count } von { $import_count } wiederhergestellt
       *[other] { $completed_count } von { $import_count } wiederhergestellt
    }
    }

journal.window.import.removal.receipt.recovered.import = Import wiederhergestellt

journal.window.import.removal.receipt.recovered.imports = { $quantity ->
        [one] { $import_count } Import wiederhergestellt
       *[other] { $import_count } Importe wiederhergestellt
    }

journal.window.import.removal.receipt.still = { $quantity ->
        [one] { $still_listed } ließ sich nicht verschieben und ist noch in deinem Journal, unten markiert.
       *[other] { $still_listed } ließen sich nicht verschieben und sind noch in deinem Journal, unten markiert.
    }

journal.window.import.removal.shared.erase = { $quantity ->
        [one] an 1 Tag bleibt ein Teil von dem, was er hereingeholt hat, und wird nicht endgültig gelöscht, weil ein früherer Import in deinem Journal es zuerst hereingeholt hat. es bleibt bei diesem Import.
       *[other] an { $shared_days } Tagen bleibt ein Teil von dem, was er hereingeholt hat, und wird nicht endgültig gelöscht, weil ein früherer Import in deinem Journal es zuerst hereingeholt hat. es bleibt bei diesem Import.
    }

journal.window.import.removal.shared.move = { $quantity ->
        [one] an 1 Tag bleibt ein Teil von dem, was er hereingeholt hat, weil ein früherer Import in deinem Journal es zuerst hereingeholt hat. es bleibt bei diesem Import.
       *[other] an { $shared_days } Tagen bleibt ein Teil von dem, was er hereingeholt hat, weil ein früherer Import in deinem Journal es zuerst hereingeholt hat. es bleibt bei diesem Import.
    }

journal.window.import.removal.waiting.many.imports = { $quantity ->
        [one] das Segment von ihnen, das für sich im Papierkorb wartet, wird mit ihnen endgültig gelöscht.
       *[other] die { $waiting_segments } Segmente von ihnen, die für sich im Papierkorb warten, werden mit ihnen endgültig gelöscht.
    }

journal.window.import.removal.waiting.one.import = { $quantity ->
        [one] das Segment von ihm, das für sich im Papierkorb wartet, wird mit ihm endgültig gelöscht.
       *[other] die { $waiting_segments } Segmente von ihm, die für sich im Papierkorb warten, werden mit ihm endgültig gelöscht.
    }

journal.window.import.response.apple.insufficient.space = dieser Import braucht etwa{ $slot1 }{ $need }{ $slot2 }frei, und diese Festplatte hat{ $slot3 }{ $free }{ $slot4 }frei, wie das System es meldet. gib etwa{ $slot5 }{ $difference } frei, dann starte ihn.

journal.window.import.response.error.a.required.field.is.missing = ein Pflichtfeld fehlt.

journal.window.import.response.error.could.not.read.progress = das Lesen des Importfortschritts ist nicht gelungen

journal.window.import.response.error.one.of.those.values.couldn.t = einer dieser Werte ließ sich nicht verwenden.

journal.window.import.response.error.something.changed.so.choose.the.source = etwas hat sich geändert, also wähle die Quelle erneut.

journal.window.import.response.error.that.action.isn.t.available.in = diese Aktion ist im aktuellen Zustand nicht verfügbar.

journal.window.import.response.error.that.archive.couldn.t.be.previewed = die Vorschau dieses Archivs ist nicht gelungen.

journal.window.import.response.error.that.delete.couldn.t.be.read = dieses Löschen ließ sich nicht lesen, also wurde nichts gelöscht.

journal.window.import.response.error.that.file.is.too.large.to = diese Datei ist zu groß, um sie hereinzuholen.

journal.window.import.response.error.that.file.isn.t.available = diese Datei ist nicht verfügbar.

journal.window.import.response.error.that.import.couldn.t.be.moved = dieser Import ließ sich nicht verschieben, also wurde nichts gelöscht.

journal.window.import.response.error.that.import.couldn.t.be.started = das Starten dieses Imports ist nicht gelungen, weil er schon vorhanden ist.

journal.window.import.response.error.that.import.isn.t.in.your = dieser Import ist nicht in deinem Journal.

journal.window.import.response.error.that.import.metadata.couldn.t.be = das Aktualisieren dieser Import-Metadaten ist nicht gelungen.

journal.window.import.response.error.that.import.metadata.couldn.t.be.variant2 = das Lesen dieser Import-Metadaten ist nicht gelungen.

journal.window.import.response.error.that.imported.content.couldn.t.be = das Lesen dieses importierten Inhalts ist nicht gelungen.

journal.window.import.response.error.the.import.could.not.be.prepared = die Vorbereitung des Imports ist nicht gelungen.

journal.window.import.response.error.there.are.no.files.to.bring = es gibt keine Dateien zum Hereinholen.

journal.window.import.response.error.there.is.nothing.new.in.this = in dieser Quelle gibt es nichts Neues zu importieren.

journal.window.import.response.error.there.s.nothing.to.browse.for = für diesen Import gibt es nichts zum Durchsehen.

journal.window.import.response.error.this.export.is.over.gb.in = dieser Export ist über 2 { $slot1 }. in { $slot2 } exportiere ihn erneut in kleineren Teilen.

journal.window.import.response.error.this.import.can.t.be.deleted = dieser Import lässt sich hier nicht löschen.

journal.window.import.response.error.this.import.has.already.started = dieser Import ist schon gestartet.

journal.window.import.response.error.this.import.or.its.source.wasn = dieser Import oder seine Quelle wurde nicht gefunden, also wähle die Quelle erneut.

journal.window.import.response.error.this.import.s.source.or.date = die Quelle oder das Datum dieses Imports ist nicht gültig.

journal.window.import.response.error.this.journal.couldn.t.be.exported = der Export dieses Journals ist nicht gelungen.

journal.window.import.response.error.this.looks.like.a.strava.download = das sieht nach einem { $slot1 }-Download aus, also hat dein Journal es nicht behalten. nutze die { $slot2 }-Karte, damit nur deine Trainings gelesen werden.

journal.window.import.response.error.your.journal.can.t.open.heic = dein Journal kann { $slot1 }- oder { $slot2 }-Fotos nicht direkt öffnen. füge es über die solstone-App auf deinem Telefon oder Computer hinzu, oder wandle es zuerst in { $slot3 } um.

journal.window.import.response.error.your.journal.couldn.t.finish.that = dein Journal konnte dieses Löschen nicht abschließen. lade neu, um zu prüfen.

journal.window.import.response.error.your.journal.couldn.t.move.that = dein Journal konnte das nicht in den Papierkorb verschieben, also wurde nichts gelöscht.

journal.window.import.response.error.your.journal.s.background.service.isn = der Hintergrunddienst deines Journals läuft nicht. starte ihn, dann versuch es noch einmal.

journal.window.import.response.evernote.count.attachment = { $quantity ->
        [one] 1 Anhang
       *[other] { $count } Anhänge
    }

journal.window.import.response.evernote.count.note = { $quantity ->
        [one] 1 Notiz
       *[other] { $count } Notizen
    }

journal.window.import.response.evernote.empty = dieser Export hat keine Notizen.

journal.window.import.response.evernote.ended.empty = dieser Export endete vorzeitig. 0 Notizen kamen an.

journal.window.import.response.evernote.finish.already.in.journal = diese Notizen sind schon in deinem Journal.

journal.window.import.response.evernote.finish.attachments = { $quantity ->
        [one] 1 Anhang kam mit ihnen an.
       *[other] { $count } Anhänge kamen mit ihnen an.
    }

journal.window.import.response.evernote.finish.attachments.single.note = { $quantity ->
        [one] 1 Anhang kam mit ihr an.
       *[other] { $count } Anhänge kamen mit ihr an.
    }

journal.window.import.response.evernote.finish.brought.in = { $quantity ->
        [one] 1 Notiz hereingeholt.
       *[other] { $count } Notizen hereingeholt.
    }

journal.window.import.response.evernote.finish.brought.in.both = { $new_part } { $changed_part }

journal.window.import.response.evernote.finish.brought.in.changed = { $quantity ->
        [one] Änderungen an 1 Notiz, die schon hier war, hereingeholt.
       *[other] Änderungen an { $count } Notizen, die schon hier waren, hereingeholt.
    }

journal.window.import.response.evernote.finish.dated.after.today = { $quantity ->
        [one] 1 Notiz ist nach heute datiert und wurde ausgelassen.
       *[other] { $count } Notizen sind nach heute datiert und wurden ausgelassen.
    }

journal.window.import.response.evernote.finish.ended.early = { $quantity ->
        [one] dieser Export endete vorzeitig. 1 Notiz kam an.
       *[other] dieser Export endete vorzeitig. { $count } Notizen kamen an.
    }

journal.window.import.response.evernote.finish.no.date = { $quantity ->
        [one] 1 Notiz hat kein Datum und wurde ausgelassen.
       *[other] { $count } Notizen haben kein Datum und wurden ausgelassen.
    }

journal.window.import.response.evernote.notebook = Notizen, die diesen Namen teilen, bleiben zusammen.

journal.window.import.response.evernote.preview.dated = { $notes }, von { $first } bis { $last }. { $attachments }.

journal.window.import.response.evernote.preview.encrypted = { $quantity ->
        [one] 1 Notiz hat einen verschlüsselten Teil, der ausgelassen wird.
       *[other] { $count } Notizen haben einen verschlüsselten Teil, der ausgelassen wird.
    }

journal.window.import.response.evernote.preview.undated = { $notes }. { $attachments }.

journal.window.import.response.evernote.type = diese Datei ist kein { $slot1 }-Export.

journal.window.import.response.originals.read = das Laden dieser Auswahl ist nicht gelungen.

journal.window.import.response.originals.unknown.source = unbekannte Importquelle

journal.window.import.response.originals.write = diese Auswahl ließ sich nicht speichern. versuch es noch einmal.

journal.window.import.response.preview.apple.go.ahead = das Speichern eines { $slot1 }-Exports braucht deine Freigabe, also wurde nichts daraus hereingeholt.{ $slot2 }{ $where }: { $slot3 }{ $slot4 }

journal.window.import.response.preview.apple.other.records = { $slot1 }-Datensätze

journal.window.import.response.preview.archive.other.files = andere Archivdateien

journal.window.import.response.preview.embedded.body = { $slot1 }-Exporte und { $slot2 }-Daten werden von einem Journal auf einem Computer hereingeholt, also wurde nichts hereingeholt.

journal.window.import.response.preview.force.day = Hinweis: diese Datei datiert auf{ $slot1 }{ $fileday }, und ein früherer Import hat{ $slot2 }{ $previous } verwendet. dieser { $slot3 }-Import wird{ $slot4 }{ $planned } verwenden. vorhandene Aufnahmen werden nicht verschoben.{ $slot5 }

journal.window.import.response.removal.set.failed = dein Journal konnte es nicht in den Papierkorb verschieben.

journal.window.import.response.removal.set.held = { $reason }.

journal.window.import.response.removal.set.not.allowed = dieser Import lässt sich hier nicht löschen.

journal.window.import.response.removal.set.unreadable = ein Teil dieses Imports ließ sich nicht lesen, also wurde nichts davon gelöscht

journal.window.import.section.heading.confirm.import.details = Importdetails bestätigen

journal.window.import.section.heading.import.history = Importverlauf

journal.window.import.section.heading.quick.import = Schnellimport

journal.window.import.section.heading.sync.sources = Abgleichquellen

journal.window.import.selection.additional = { $quantity ->
        [one] und { $additional_count } mehr
       *[other] und { $additional_count } mehr
    }

journal.window.import.selection.count = { $quantity ->
        [one] { $selected_count } ausgewählt
       *[other] { $selected_count } ausgewählt
    }

journal.window.import.selection.file.name = wähle { $file_name }

journal.window.import.source.apple.health.description = importiere deine Trainings, deinen Schlaf und deine Messwerte aus einem { $slot1 }-Export

journal.window.import.source.apple.health.name = { $slot1 }

journal.window.import.source.apple.health.upload.prompt = lade export.zip aus der Health-App auf deinem iPhone hoch

journal.window.import.source.audio.archive.description = hol { $slot1 }-Aufnahmen mit ihren ursprünglichen Aufnahmezeiten herein

journal.window.import.source.audio.archive.name = Audio-Archiv

journal.window.import.source.audio.archive.upload.prompt = füge den vollständigen Pfad zu deinem Aufnahmeordner ein

journal.window.import.source.chatgpt.description = importiere deinen Gesprächsverlauf aus { $slot1 }

journal.window.import.source.chatgpt.name = { $slot1 }

journal.window.import.source.chatgpt.upload.prompt = lade deine { $slot1 }-Export-{ $slot2 }-Datei hoch

journal.window.import.source.claude.description = importiere deinen Gesprächsverlauf aus { $slot1 }

journal.window.import.source.claude.name = { $slot1 }

journal.window.import.source.claude.upload.prompt = lade deine { $slot1 }-Export-{ $slot2 }-Datei hoch

journal.window.import.source.document.description = importiere ein { $slot1 }-Dokument

journal.window.import.source.document.name = Dokument

journal.window.import.source.document.upload.prompt = lade eine { $slot1 }-Datei hoch

journal.window.import.source.evernote.description = hol Notizen aus einem { $slot1 }-Export herein

journal.window.import.source.evernote.name = { $slot1 }

journal.window.import.source.evernote.upload.prompt = lade einen { $slot1 }-Export hoch

journal.window.import.source.gemini.description = importiere deinen Aktivitätsverlauf aus Google { $slot1 }

journal.window.import.source.gemini.name = { $slot1 }

journal.window.import.source.gemini.upload.prompt = lade deine { $slot1 }-{ $slot2 }-Datei hoch

journal.window.import.source.ics.description = importiere Termine aus { $slot1 }, { $slot2 } oder { $slot3 }

journal.window.import.source.ics.name = Kalender

journal.window.import.source.ics.upload.prompt = lade deine { $slot1 }-Datei oder den { $slot2 }-Export hoch

journal.window.import.source.image.description = füge ein Foto oder Bildschirmfoto hinzu und lass ein Modell beschreiben, was darin ist

journal.window.import.source.image.name = Bild

journal.window.import.source.image.upload.prompt = lade ein Bild hoch ({ $slot1 }, { $slot2 }, { $slot3 }, { $slot4 })

journal.window.import.source.journal.archive.description = importiere einen vollständigen Export des Journals aus einem anderen Journal

journal.window.import.source.journal.archive.name = Journal

journal.window.import.source.journal.archive.upload.prompt = lade deine Journal-Export-{ $slot1 }-Datei hoch

journal.window.import.source.quick.description = füge Text ein oder leg eine beliebige Datei für den Schnellimport ab

journal.window.import.source.quick.name = Schnellimport

journal.window.import.source.quick.upload.prompt = füge Text ein oder zieh eine Datei und leg sie ab

journal.window.import.source.recording.description = importiere Audio aus Besprechungen oder Gesprächen

journal.window.import.source.recording.name = Besprechungs-Audio

journal.window.import.source.recording.upload.prompt = lade eine Audio-, Video- oder Bilddatei hoch

journal.window.import.source.strava.description = importiere deine Trainings aus einem { $slot1 }-Download

journal.window.import.source.strava.name = { $slot1 }

journal.window.import.source.strava.upload.prompt = lade deinen { $slot1 }-Download { $slot2 } hoch, oder nur { $slot3 } daraus

journal.window.import.stage.names.audio.segmenting.splitting.audio = Audio wird aufgeteilt…

journal.window.import.stage.names.cleanup.finishing.up = wird fertig…

journal.window.import.stage.names.detection.detecting.content.type = Inhaltstyp wird erkannt…

journal.window.import.stage.names.entities.merging.entities = Entitäten werden zusammengeführt…

journal.window.import.stage.names.execution.reading.the.import = Import wird gelesen…

journal.window.import.stage.names.facets.merging.facets = Facetten werden zusammengeführt…

journal.window.import.stage.names.importing.reading.the.import = Import wird gelesen…

journal.window.import.stage.names.imports.copying.prior.imports = vorherige Importe werden kopiert…

journal.window.import.stage.names.indexing.building.search.index = Suchindex wird aufgebaut…

journal.window.import.stage.names.initialization.preparing = wird vorbereitet…

journal.window.import.stage.names.processing.processing.entries = Einträge werden verarbeitet…

journal.window.import.stage.names.segmenting.preparing.segments = Segmente werden vorbereitet…

journal.window.import.stage.names.segments.merging.segments = Segmente werden zusammengeführt…

journal.window.import.stage.names.transcribing.transcribing.audio = Audio wird transkribiert…

journal.window.import.stage.names.transcription.transcribing.audio = Audio wird transkribiert…

journal.window.import.stage.names.writing.writing.to.journal = wird ins Journal geschrieben…

journal.window.import.status.completed = abgeschlossen

journal.window.import.status.conflict.already.imported = du hast diese Datei schon importiert. hast du einen Teil davon gelöscht, stell ihn aus dem Papierkorb wieder her, oder lösche den ganzen Import und importiere sie erneut.

journal.window.import.status.conflict.already.started = dieser Import ist schon gestartet. prüfe den Fortschritt in deinen Importen.

journal.window.import.status.conflict.changed = dieser Import hat sich geändert. lade die Seite neu und versuch es noch einmal.

journal.window.import.status.error.failed = Import ist nicht gelungen

journal.window.import.status.error.never.completed = Import wurde nie abgeschlossen

journal.window.import.status.error.unavailable = Importstatus nicht verfügbar

journal.window.import.status.error.unconfirmed = dieser Import ließ sich nicht als abgeschlossen bestätigen.

journal.window.import.status.error.with.stage = { $error } ({ $stage })

journal.window.import.status.failed = nicht gelungen

journal.window.import.status.pending = ausstehend

journal.window.import.status.running = läuft

journal.window.import.status.stage.execution = Ausführung

journal.window.import.status.stage.finalization = Abschluss

journal.window.import.status.stage.interrupted = Unterbrochen

journal.window.import.status.stage.publication = Veröffentlichung

journal.window.import.status.stage.storage = Speicher

journal.window.import.status.stage.timeout = Zeitüberschreitung

journal.window.import.status.status.unavailable = Status nicht verfügbar

journal.window.import.status.stopped = gestoppt

journal.window.import.status.text.any.supported.file = jede unterstützte Datei

journal.window.import.status.text.failed = nicht gelungen

journal.window.import.status.text.paste.the.full.path.to.your = füge den vollständigen Pfad zu deinem Ordner ein

journal.window.import.status.text.please.provide.a.file.or.paste = Bitte gib eine Datei an oder füge Text ein

journal.window.import.status.text.select.a.file.to.import = wähle eine Datei zum Importieren

journal.window.import.status.text.the.date.is.required.before.starting = das Datum ist erforderlich, bevor der Import gestartet wird.

journal.window.import.status.text.unknown.error = unbekannter Fehler

journal.window.import.status.text.validation.failed = Validierung ist nicht gelungen

journal.window.import.status.unconfirmed = unbestätigt

journal.window.import.strings.activities.activities = Aktivitäten

journal.window.import.strings.artifact.paths.artifact.paths = Artefaktpfade

journal.window.import.strings.body.didnt.start.didn.t.start = nicht gestartet

journal.window.import.strings.body.failed.not.brought.in = nicht hereingeholt

journal.window.import.strings.body.partly.saved.partly.saved = teilweise gespeichert

journal.window.import.strings.body.stopped.stopped = gestoppt

journal.window.import.strings.check.status.check.status = Status prüfen

journal.window.import.strings.checking.status.checking.status = Status wird geprüft…

journal.window.import.strings.collision.title.owner.identity.differs.between.journals = deine Identität unterscheidet sich zwischen den Journalen

journal.window.import.strings.completed.at.completed.at = abgeschlossen am

journal.window.import.strings.completed.completed = abgeschlossen

journal.window.import.strings.created.files.created.files = erstellte Dateien

journal.window.import.strings.date.range.date.range = Datumsbereich

journal.window.import.strings.date.source.date.source = Datumsquelle

journal.window.import.strings.date.source.fallback.import.time.no.date.in.photo = Importzeit (kein Datum im Foto)

journal.window.import.strings.date.source.owner.chosen.by.you = von dir gewählt

journal.window.import.strings.date.source.photo.from.photo = vom Foto

journal.window.import.strings.decisions.decisions = Entscheidungen

journal.window.import.strings.delete.import.delete.this.import = diesen Import löschen

journal.window.import.strings.detected.time.detected.time = erkannte Zeit

journal.window.import.strings.drawer.label.bookkeeping = Buchführung

journal.window.import.strings.drawer.unavailable.drawer.renderer.unavailable = Renderer der Schublade nicht verfügbar

journal.window.import.strings.entities.entities = Entitäten

journal.window.import.strings.entries.entries = Einträge

journal.window.import.strings.error.error = Fehler

journal.window.import.strings.facets.facets = Facetten

journal.window.import.strings.failed.at.failed.at = nicht gelungen am

journal.window.import.strings.failed.failed = nicht gelungen

journal.window.import.strings.failed.line.failed.while.processing = bei der Verarbeitung nicht gelungen

journal.window.import.strings.failed.stage.failed.stage = nicht gelungene Phase

journal.window.import.strings.file.file = Datei

journal.window.import.strings.file.size.file.size = Dateigröße

journal.window.import.strings.files.created.files.created = Dateien erstellt

journal.window.import.strings.files.files = Dateien

journal.window.import.strings.has.gaps.contains.gaps = enthält Lücken

journal.window.import.strings.identity.differs.identity.differs = Identität unterscheidet sich

journal.window.import.strings.image.description.gap.detail.image.saved.without.visual.description = Bild ohne visuelle Beschreibung gespeichert

journal.window.import.strings.import.time.import.time = Importzeit

journal.window.import.strings.imports.imports = Importe

journal.window.import.strings.leads.title.where.this.landed = wo das gelandet ist

journal.window.import.strings.merge.highlights.merge.highlights = Höhepunkte der Zusammenführung

journal.window.import.strings.merge.summary.merge.summary = Zusammenfassung der Zusammenführung

journal.window.import.strings.mime.type.mime.type = MIME-Typ

journal.window.import.strings.not.started.not.started = nicht gestartet

journal.window.import.strings.nothing.to.import.nothing.to.import = nichts zu importieren

journal.window.import.strings.original.file.original.file = Originaldatei

journal.window.import.strings.owner.identity.differs.owner.identity.differs = deine Identität unterscheidet sich

journal.window.import.strings.pending.pending = ausstehend

journal.window.import.strings.photo.fallback.notice.no.date.in.photo.placed.at = kein Datum im Foto · zur Importzeit platziert

journal.window.import.strings.preparing.preview.preparing.preview = Vorschau wird vorbereitet…

journal.window.import.strings.preview.failed.couldn.t.prepare.a.preview.try = eine Vorschau ließ sich nicht vorbereiten. versuch es noch einmal.

journal.window.import.strings.preview.import.preview.import = Importvorschau

journal.window.import.strings.processing.facts.processing.facts = Fakten der Verarbeitung

journal.window.import.strings.processing.processing = wird verarbeitet…

journal.window.import.strings.profile.link.review.profile.settings = Profileinstellungen prüfen

journal.window.import.strings.raw.payload.raw.payload = rohe Nutzdaten

journal.window.import.strings.running.running = läuft

journal.window.import.strings.saved.saved = gespeichert

journal.window.import.strings.segments.segments = Segmente

journal.window.import.strings.setting.setting = Kontext

journal.window.import.strings.source.source = Quelle

journal.window.import.strings.staged.entities.staged.entities = bereitgestellte Entitäten

journal.window.import.strings.staging.staging = Bereitstellung

journal.window.import.strings.start.import.start.import = Import starten

journal.window.import.strings.start.unconfirmed.couldn.t.confirm.whether.the.import = es ließ sich nicht bestätigen, ob der Import gestartet wurde. prüfe den Status, bevor du etwas anderes tust.

journal.window.import.strings.start.unknown.detail.this.file.is.saved.but.its = diese Datei ist gespeichert, aber ihr Verlauf bestätigt nicht, ob der Import gestartet wurde.

journal.window.import.strings.start.unknown.start.status.unknown = Startstatus unbekannt

journal.window.import.strings.starting.starting = wird gestartet…

journal.window.import.strings.status.failed.couldn.t.load.the.import.s = der Status des Imports ließ sich nicht laden. prüfe erneut.

journal.window.import.strings.status.status = Status

journal.window.import.strings.status.unavailable.import.status.unavailable = Importstatus nicht verfügbar

journal.window.import.strings.summary.errors.summary.errors = Fehler der Zusammenfassung

journal.window.import.strings.target.day.target.day = Zieltag

journal.window.import.strings.unavailable.description.unavailable.description = Beschreibung nicht verfügbar

journal.window.import.strings.unavailable.pages.unavailable.pages = Seiten nicht verfügbar

journal.window.import.strings.unconfirmed.explanation.this.import.couldn.t.be.confirmed = dieser Import ließ sich nicht als abgeschlossen bestätigen.

journal.window.import.strings.unconfirmed.import.unconfirmed = Import unbestätigt

journal.window.import.strings.untimed.ask.ask.us.to.add.it = frag uns, es hinzuzufügen

journal.window.import.strings.untimed.notice.no.turn.times.found.in.this = keine Zeiten von Redebeiträgen in dieser Datei gefunden, daher ist sie als ein Segment zur Startzeit des Imports gelandet, jede Zeile wie geschrieben. wenn deine Transkripte Zeiten auf eine Weise markieren, die das Journal noch nicht liest,

journal.window.import.strings.upload.facts.upload.facts = Fakten des Hochladens

journal.window.import.strings.uploaded.at.uploaded.at = hochgeladen am

journal.window.import.strings.view.day.view.that.day = diesen Tag ansehen

journal.window.import.unit.record = Datensatz

journal.window.import.unit.records = Datensätze

journal.window.import.upload.accepted.formats = akzeptiert { $formats }

journal.window.import.upload.duplicate.running = dieser Import läuft bereits.

journal.window.import.upload.duplicate.running.named = dieser Import läuft bereits als { $import_id }.

journal.window.import.upload.duplicate.saved = diese Datei war bereits bereitgestellt.

journal.window.import.upload.duplicate.saved.named = diese Datei war bereits als { $import_id } bereitgestellt.

journal.window.import.upload.saved.named = { $name }: gespeichert

journal.window.import.upload.saved.setup.unconfirmed = { $name }: gespeichert, aber die Import-Einrichtung ließ sich nicht bestätigen. prüfe deine Importe, bevor du es erneut wählst.

journal.window.import.visible.text.a.short.note.about.what.this = eine kurze Notiz, worum es geht, damit du es später leichter findest

journal.window.import.visible.text.checking.import = Import wird geprüft…

journal.window.import.visible.text.completed = abgeschlossen

journal.window.import.visible.text.configure.and.start = konfigurieren und starten

journal.window.import.visible.text.contains.gaps = enthält Lücken

journal.window.import.visible.text.content = Inhalt

journal.window.import.visible.text.couldn.t.check.status.reload.to = der Status ließ sich nicht prüfen. lade neu und versuch es noch einmal.

journal.window.import.visible.text.couldn.t.load.import.details = die Importdetails ließen sich nicht laden.

journal.window.import.visible.text.couldn.t.load.more.imports = weitere Importe ließen sich nicht laden.

journal.window.import.visible.text.couldn.t.load.this.choice.reopen = diese Auswahl ließ sich nicht laden. öffne den Import erneut und versuch es noch einmal.

journal.window.import.visible.text.couldn.t.save.this.choice.try = diese Auswahl ließ sich nicht speichern. versuch es noch einmal.

journal.window.import.visible.text.date.range = Datumsbereich:

journal.window.import.visible.text.drop.a.file.here.or.choose = lege eine Datei hier ab oder wähle eine aus

journal.window.import.visible.text.duration = Dauer:

journal.window.import.visible.text.entities.seeded = Entitäten angelegt:

journal.window.import.visible.text.entries.written = Einträge geschrieben:

journal.window.import.visible.text.error = Fehler:

journal.window.import.visible.text.error.loading.content = Fehler beim Laden des Inhalts

journal.window.import.visible.text.error.loading.content.variant2 = Fehler beim Laden des Inhalts

journal.window.import.visible.text.folder.saved.but.import.setup.could = Ordner gespeichert, aber die Import-Einrichtung ließ sich nicht bestätigen. prüfe deine Importe, bevor du ihn erneut wählst.

journal.window.import.visible.text.import.id = Import-ID:

journal.window.import.visible.text.leave.this.empty.to.use.recording = lass das leer, um die Dateinamen der Aufnahmen zu verwenden. ein Manifest und seine Aufnahmen müssen zusammen im ausgewählten Ordner liegen. das Manifest kann eigene Zeiten angeben.

journal.window.import.visible.text.loading = wird geladen…

journal.window.import.visible.text.loading.content = Inhalt wird geladen…

journal.window.import.visible.text.loading.imports = Importe werden geladen…

journal.window.import.visible.text.no.content.available = Kein Inhalt verfügbar

journal.window.import.visible.text.no.creation.date.was.found.in = in dieser Datei wurde kein Erstellungsdatum gefunden. wähle ihr Datum oder behalte das Hochladedatum.

journal.window.import.visible.text.no.items.found = keine Elemente gefunden

journal.window.import.visible.text.optional = (optional)

journal.window.import.visible.text.original.saved.description.unavailable = Original gespeichert (Beschreibung nicht verfügbar)

journal.window.import.visible.text.overview = Übersicht

journal.window.import.visible.text.overview.unavailable = Übersicht nicht verfügbar

journal.window.import.visible.text.paste.text.or.drop.a.file = füge Text ein oder lege eine Datei ab. du bestätigst das Datum, bevor es hineingeht.

journal.window.import.visible.text.preparing.preview = Vorschau wird vorbereitet…

journal.window.import.visible.text.preview.import = Importvorschau

journal.window.import.visible.text.preview.then.merge.your.archive = Vorschau anzeigen, dann dein Archiv zusammenführen

journal.window.import.visible.text.preview.your.recordings = Vorschau deiner Aufnahmen

journal.window.import.visible.text.progress = Fortschritt

journal.window.import.visible.text.ready.to.sync.recordings.into.your = bereit, Aufnahmen in dein Journal abzugleichen.

journal.window.import.visible.text.saving.folder.this.can.take.a = Ordner wird gespeichert… das kann bei einem großen Ordner eine Weile dauern.

journal.window.import.visible.text.select.this.tab.to.browse.imported = wähle diesen Tab, um importierten Inhalt durchzusehen

journal.window.import.visible.text.stalled = stockt

journal.window.import.visible.text.stalled.no.updates.in.minutes = stockt: seit 10 Minuten keine Aktualisierung.

journal.window.import.visible.text.start.import = Import starten

journal.window.import.visible.text.starting = wird gestartet

journal.window.import.visible.text.status.unavailable = Status nicht verfügbar

journal.window.import.visible.text.still.working = arbeitet noch…

journal.window.import.visible.text.the.zip.includes.your.settings.and = die ZIP enthält deine Einstellungen und Zugangsdaten. Verarbeitungsprotokolle, neu aufbaubare Indizes und Zwischenspeicher, Sperren und temporäre Dateien sind ausgelassen. das Zusammenführen behält die Einstellungen und Verbindungen des empfangenden Journals.

journal.window.import.visible.text.this.import.could.not.be.confirmed = dieser Import ließ sich nicht bestätigen. prüfe deine Importe, bevor du ihn erneut wählst.

journal.window.import.visible.text.this.import.couldn.t.be.confirmed = dieser Import ließ sich nicht als abgeschlossen bestätigen.

journal.window.import.visible.text.this.pdf.uses.its.own.dates = dieses { $slot1 } nutzt eigene Datumsangaben. du kannst ein Datum für diese Datei wählen.

journal.window.import.visible.text.turn.this.on.if.you.want = schalte das ein, wenn du eine lokale Kopie der Originale in deinem Journal willst.

journal.window.import.visible.text.unconfirmed = unbestätigt

journal.window.import.visible.text.unknown.import.source = unbekannte Importquelle

journal.window.import.visible.text.upload.your.file = deine Datei hochladen

journal.window.import.visible.text.uploading.your.file.keep.this.page = deine Datei wird hochgeladen… halte diese Seite offen, bis sie fertig ist.

journal.window.import.visible.text.validate = überprüfen

journal.window.import.visible.text.validating = wird überprüft

journal.window.import.window.explanation.could.not.preview.this.import = dieser Import ließ sich nicht in der Vorschau anzeigen

journal.window.import.window.explanation.failed.to.save.import = Import ließ sich nicht speichern

journal.window.import.window.explanation.failed.to.save.import.settings = Importeinstellungen ließen sich nicht speichern

journal.window.import.window.explanation.failed.to.start.import = Import ließ sich nicht starten

journal.window.import.window.explanation.malformed.import.history.response = Fehlerhafte Antwort des Importverlaufs

journal.window.import.window.explanation.stalled.no.updates.in.minutes = stockt: seit 10 Minuten keine Aktualisierung

journal.window.import.window.explanation.thinking.progress.is.unavailable = Fortschritt des Denkens ist nicht verfügbar

journal.window.ingest.notice.app.newer = seine solstone-App ist neuer als dein Journal. aktualisiere dein Journal.

journal.window.ingest.notice.app.older = seine solstone-App ist zu alt für dein Journal. aktualisiere die solstone-App darauf.

journal.window.ingest.notice.cleared.named = dein Journal nimmt wieder Uploads von { $device_name } entgegen

journal.window.ingest.notice.cleared.unnamed = dein Journal nimmt wieder Uploads von einem deiner Geräte entgegen

journal.window.ingest.notice.raised.named = dein Journal weist Uploads von { $device_name } ab

journal.window.ingest.notice.raised.unnamed = dein Journal weist Uploads von einem deiner Geräte ab

journal.window.ingest.notice.tmux.newer = solstone für tmux darauf ist neuer als dein Journal. aktualisiere dein Journal.

journal.window.ingest.notice.tmux.older = solstone für tmux darauf ist zu alt für dein Journal. aktualisiere solstone für tmux.

journal.window.link.absent = es ist nicht in deinem Journal.

journal.window.link.back = ← zurück

journal.window.link.heading = diese Quelle lässt sich nicht öffnen

journal.window.link.invalid = dein Journal folgt diesem Link nicht.

journal.window.link.open.journal = dein Journal öffnen

journal.window.link.read.error = dein Journal konnte diese Quelle nicht prüfen.

journal.window.link.try.again = erneut versuchen

journal.window.link.unsupported = dein Journal kann diese Art von Quelle nicht anzeigen.

journal.window.link.wrong.journal.detail = es kann für ein anderes Journal bestimmt sein, oder der Link kann sich geändert haben.

journal.window.link.wrong.journal.heading = dieser Link lässt sich hier nicht öffnen.

journal.window.stored.body.summary.activities.no.facet =     { $quantity ->
        [one] 1 Training, eine Schlafnacht oder eine Achtsamkeitssitzung ist nicht auf der Seite Aktivitäten aufgeführt, weil dieser Teil deines Journals keine Facette hat.
       *[other] { $count } der Trainings, Schlafnächte und Achtsamkeitssitzungen sind nicht auf der Seite Aktivitäten aufgeführt, weil diese Teile deines Journals keine Facette haben.
    }

journal.window.stored.body.summary.activities.refreshed =     { $quantity ->
        [one] 1 Training, eine Schlafnacht oder eine Achtsamkeitssitzung hat in diesem Import die Zeiten oder die Art geändert und zeigt jetzt die neuen.
       *[other] { $count } der Trainings, Schlafnächte und Achtsamkeitssitzungen haben in diesem Import die Zeiten oder die Art geändert und zeigen jetzt die neuen.
    }

journal.window.stored.body.summary.activities.refused = Einige Trainings, Schlafnächte oder Achtsamkeitssitzungen ließen sich unter { $facets } nicht anzeigen. Der nächste Import versucht es erneut.

journal.window.stored.body.summary.activities.shown =     { $quantity ->
        [one] 1 Training, eine Schlafnacht oder eine Achtsamkeitssitzung ist jetzt am zugehörigen Tag zu sehen.
       *[other] { $count } der Trainings, Schlafnächte und Achtsamkeitssitzungen sind jetzt an den zugehörigen Tagen zu sehen.
    }

journal.window.stored.body.summary.counts.base = { $written } geschrieben, { $unchanged } unverändert, { $unreadable } unlesbar und unangetastet.

journal.window.stored.body.summary.counts.stayed = { $written } geschrieben, { $unchanged } unverändert, { $stayed_deleted } gelöscht geblieben, { $unreadable } unlesbar und unangetastet.

journal.window.stored.body.summary.deferred =     { $quantity ->
        [one] { $source } hat 1 Eintrag auf die andere Seite der Mitternacht verschoben; dein Journal behält die Zeit, die es zuerst gespeichert hat.
       *[other] { $source } hat { $count } Einträge auf die andere Seite der Mitternacht verschoben; dein Journal behält die Zeiten, die es zuerst gespeichert hat.
    }

journal.window.stored.body.summary.earlier.waiting =     { $quantity ->
        [one] 1 früheres Strava-Training ließ sich noch nicht aktualisieren, daher kann es zweimal neben dem, was dieser Import hinzugefügt hat, erscheinen, bis es aktualisiert ist.
       *[other] { $count } frühere Strava-Trainings ließen sich noch nicht aktualisieren, daher können sie zweimal neben dem, was dieser Import hinzugefügt hat, erscheinen, bis sie aktualisiert sind.
    }

journal.window.stored.body.summary.entries.stayed.deleted =     { $quantity ->
        [one] 1 Eintrag wurde nicht hinzugefügt, weil du den Teil des Journals, zu dem er gehört, gelöscht hast.
       *[other] { $count } Einträge wurden nicht hinzugefügt, weil du den Teil des Journals, zu dem sie gehören, gelöscht hast.
    }

journal.window.stored.body.summary.facet.chosen = neue Einträge werden unter der Facette { $facet } eingeordnet.

journal.window.stored.body.summary.facet.none = keine Facette wurde für diesen Import gewählt. Einträge ohne Facette sind für einen Agenten lesbar, der dein ganzes Journal lesen kann, und nicht für einen, der auf gewählte Facetten beschränkt ist.

journal.window.stored.body.summary.facets.another = eine andere Facette

journal.window.stored.body.summary.facets.earlier.only = eine frühere Einordnung

journal.window.stored.body.summary.facets.many = deinen Facetten { $rest } und { $last }

journal.window.stored.body.summary.facets.one = deiner Facette { $facet }

journal.window.stored.body.summary.facets.with.earlier = { $named } und einer früheren Einordnung

journal.window.stored.body.summary.heading = # { $source }-Import

journal.window.stored.body.summary.held.back.reading =     { $quantity ->
        [one] 1 Messwert wurde nicht hinzugefügt, weil der Teil des Journals, zu dem er gehört, nicht gelesen werden konnte.
       *[other] { $count } Messwerte wurden nicht hinzugefügt, weil der Teil des Journals, zu dem sie gehören, nicht gelesen werden konnte.
    }

journal.window.stored.body.summary.hole.converging = - { $endpoint }, { $place }: seitdem in Oura geändert, und das Journal hat das Verschieben an die neue Zeit noch nicht abgeschlossen; ein Import, der diesen Tag erneut anfordert, schließt es ab

journal.window.stored.body.summary.hole.deferred = - { $endpoint }, { $place }: Oura hat das auf die andere Seite der Mitternacht verschoben; dein Journal behält die Zeit, die es zuerst gespeichert hat

journal.window.stored.body.summary.hole.held.back = - { $endpoint }, { $place }: noch nicht hinzugefügt: es könnte schon in einem Teil des Journals sein, der nicht gelesen werden konnte; ein Import, der diesen Tag erneut anfordert, versucht es noch einmal

journal.window.stored.body.summary.hole.moved.far = - { $endpoint }, { $place }: Oura hat das um mehr als einen Tag verschoben; dein Journal behält es dort, wo es zuerst gespeichert wurde

journal.window.stored.body.summary.hole.record.refused = - { $endpoint }, { $place }: es konnte an seinem Tag nicht angezeigt werden; ein Import, der diesen Tag erneut anfordert, versucht es noch einmal

journal.window.stored.body.summary.hole.unreadable = - { $endpoint }, { $place }: der Teil des Journals, in den es gehört, konnte nicht gelesen werden; ein Import, der diesen Tag erneut anfordert, versucht es noch einmal

journal.window.stored.body.summary.joined.hidden =     { $quantity ->
        [one] 1 Datensatz ist zu einem Training dazugekommen, das du verborgen hattest, also ist er auch verborgen.
       *[other] { $count } Datensätze sind zu einem Training dazugekommen, das du verborgen hattest, also sind sie auch verborgen.
    }

journal.window.stored.body.summary.kept.facet =     { $quantity ->
        [one] 1 Teil deines Journals trägt schon eine Facette aus einem früheren Import, also ist das, was dieser Import hinzugefügt hat, auch unter dieser Facette lesbar.
       *[other] { $count } Teile deines Journals tragen schon eine Facette aus einem früheren Import, also ist das, was dieser Import hinzugefügt hat, auch unter dieser Facette lesbar.
    }

journal.window.stored.body.summary.kept.unfiled = einige frühere Einträge blieben ohne Facette. nur ein Agent, der dein ganzes Journal lesen kann, kann sie lesen.

journal.window.stored.body.summary.listed.moved =     { $quantity ->
        [one] 1 Datensatz, der schon auf der Seite Aktivitäten aufgelistet war, wurde zur Facette dieses Imports verschoben, von { $facets }.
       *[other] { $count } Datensätze, die schon auf der Seite Aktivitäten aufgelistet waren, wurden zur Facette dieses Imports verschoben, von { $facets }.
    }

journal.window.stored.body.summary.listed.over.none.none.shown =     { $quantity ->
        [one] nichts, was schon aufgelistet war, wurde verschoben: 1 Datensatz wäre verschoben worden, und dir wurde keiner angezeigt.
       *[other] nichts, was schon aufgelistet war, wurde verschoben: { $count } Datensätze wären verschoben worden, und dir wurde keiner angezeigt.
    }

journal.window.stored.body.summary.listed.over.none.shown =     { $quantity ->
        [one] nichts, was schon aufgelistet war, wurde verschoben: 1 Datensatz wäre verschoben worden, und dir wurde { $shown } mitgeteilt.
       *[other] nichts, was schon aufgelistet war, wurde verschoben: { $count } Datensätze wären verschoben worden, und dir wurde { $shown } mitgeteilt.
    }

journal.window.stored.body.summary.listed.over.some.none.shown =     { $quantity ->
        [one] der Rest dessen, was schon aufgelistet ist, blieb, wo er war: insgesamt wäre 1 Datensatz verschoben worden, und dir wurde keiner angezeigt.
       *[other] der Rest dessen, was schon aufgelistet ist, blieb, wo er war: insgesamt wären { $count } Datensätze verschoben worden, und dir wurde keiner angezeigt.
    }

journal.window.stored.body.summary.listed.over.some.shown =     { $quantity ->
        [one] der Rest dessen, was schon aufgelistet ist, blieb, wo er war: insgesamt wäre 1 Datensatz verschoben worden, und dir wurde { $shown } mitgeteilt.
       *[other] der Rest dessen, was schon aufgelistet ist, blieb, wo er war: insgesamt wären { $count } Datensätze verschoben worden, und dir wurde { $shown } mitgeteilt.
    }

journal.window.stored.body.summary.metric.daily.activity = tägliche Aktivität

journal.window.stored.body.summary.metric.daily.cardiovascular.age = kardiovaskuläres Alter

journal.window.stored.body.summary.metric.daily.readiness = Bereitschaft

journal.window.stored.body.summary.metric.daily.resilience = Resilienz

journal.window.stored.body.summary.metric.daily.sleep = Schlafwerte

journal.window.stored.body.summary.metric.daily.spo2 = Blutsauerstoff

journal.window.stored.body.summary.metric.daily.stress = Stress

journal.window.stored.body.summary.metric.heartrate = Herzfrequenz

journal.window.stored.body.summary.metric.rest.mode.period = Ruhephasen

journal.window.stored.body.summary.metric.ring.configuration = Ringdetails

journal.window.stored.body.summary.metric.session = Sitzungen

journal.window.stored.body.summary.metric.sleep.time = vorgeschlagene Schlafenszeiten

journal.window.stored.body.summary.metric.tags = Schlagwörter

journal.window.stored.body.summary.metric.vo2.max = VO2 max

journal.window.stored.body.summary.metric.workout = Trainings

journal.window.stored.body.summary.moved =     { $quantity ->
        [one] { $source } hat seitdem die Zeit von 1 Eintrag geändert; das Journal zeigt ihn jetzt zur neuen Zeit.
       *[other] { $source } hat seitdem die Zeiten von { $count } Einträgen geändert; das Journal zeigt sie jetzt zu den neuen Zeiten.
    }

journal.window.stored.body.summary.moved.far =     { $quantity ->
        [one] { $source } hat 1 Eintrag um mehr als einen Tag verschoben; dein Journal behält ihn dort, wo er zuerst gespeichert wurde.
       *[other] { $source } hat { $count } Einträge um mehr als einen Tag verschoben; dein Journal behält sie dort, wo sie zuerst gespeichert wurden.
    }

journal.window.stored.body.summary.oura.days.heartrate = { $from } bis { $to }

journal.window.stored.body.summary.oura.days.other = Ouras Tage { $from } bis { $to }

journal.window.stored.body.summary.oura.empty = - { $endpoint }: leer: nichts für { $days }

journal.window.stored.body.summary.oura.failed = - { $endpoint }: nicht gelungen: { $error }

journal.window.stored.body.summary.oura.failed.part.way = - { $endpoint }: auf halbem Weg nicht gelungen: vollständig gelesen bis { $through }, dann { $error }

journal.window.stored.body.summary.oura.read.entries =     { $quantity ->
        [one] - { $endpoint }: gelesen: 1 Eintrag für { $days }
       *[other] - { $endpoint }: gelesen: { $count } Einträge für { $days }
    }

journal.window.stored.body.summary.oura.read.heartrate =     { $quantity ->
        [one] - { $endpoint }: gelesen: 1 Messwert für { $days }
       *[other] - { $endpoint }: gelesen: { $count } Messwerte für { $days }
    }

journal.window.stored.body.summary.oura.refusal.documented = das braucht die Berechtigung "{ $scope }"; um es hereinzuholen, verbinde Oura erneut und erlaube sie

journal.window.stored.body.summary.oura.refusal.named = Oura sagt, dass das die Berechtigung { $scopes } braucht; um es hereinzuholen, verbinde Oura erneut und erlaube sie

journal.window.stored.body.summary.oura.refusal.not.named = Oura hat nicht gesagt, welche Berechtigung das braucht

journal.window.stored.body.summary.oura.refusal.said = Oura hat gesagt: { $detail }

journal.window.stored.body.summary.oura.refused = - { $endpoint }: abgelehnt: { $why }

journal.window.stored.body.summary.oura.refused.part.way = - { $endpoint }: auf halbem Weg abgelehnt: vollständig gelesen bis { $through }, dann { $why }

journal.window.stored.body.summary.oura.through.day = Ouras Tag { $day }

journal.window.stored.body.summary.overlapping.nights = { $count } von Ouras Nächten in diesem Import überlappen sich mit einer anderen. jede wird so behalten, wie Oura sie gegeben hat, also werden beide angezeigt, wenn Oura eine geänderte Nacht als neue abgelegt hat.

journal.window.stored.body.summary.placed.zone = Tage und Zeiten gesetzt anhand von { $zone }.

journal.window.stored.body.summary.refiled =     { $quantity ->
        [one] 1 Teil deines Journals hielt schon Datensätze, und die sind jetzt unter dieser Facette abgelegt statt unter der, die sie hatten.
       *[other] { $count } Teile deines Journals hielten schon Datensätze, und die sind jetzt unter dieser Facette abgelegt statt unter der, die sie hatten.
    }

journal.window.stored.body.summary.replaced = Dieser Import wurde von einem späteren ersetzt, bevor er fertig war.

journal.window.stored.body.summary.retained.revisions =     { $quantity ->
        [one] 1 aktualisierter Eintrag blieb unter { $facets }, wo er schon abgelegt war.
       *[other] { $count } aktualisierte Einträge blieben unter { $facets }, wo sie schon abgelegt waren.
    }

journal.window.stored.body.summary.section.couldnt.be.read = ## konnte nicht gelesen werden

journal.window.stored.body.summary.section.from.oura = ## Von Oura

journal.window.stored.body.summary.section.held.back = ## Zurückgehalten

journal.window.stored.body.summary.section.held.back.intro = Noch nicht hinzugefügt: jedes könnte schon in einem Teil des Journals sein, der nicht gelesen werden konnte. Der nächste Import versucht es noch einmal.

journal.window.stored.body.summary.section.not.read = ## nicht gelesen

journal.window.stored.body.summary.section.not.saved.yet = ## noch nicht gespeichert

journal.window.stored.body.summary.section.not.written.in.full = ## Nicht vollständig geschrieben

journal.window.stored.body.summary.section.not.written.in.full.intro = Oura hat für diese geantwortet, und das Journal konnte nicht alles davon schreiben.

journal.window.stored.body.summary.section.read = ## gelesen

journal.window.stored.body.summary.split.rows =     { $quantity ->
        [one] 1 neuer Eintrag wurde getrennt von früheren Einträgen im selben Zeitraum gespeichert, sodass jeder seine eigene Facette behält.
       *[other] { $count } neue Einträge wurden getrennt von früheren Einträgen im selben Zeitraum gespeichert, sodass jeder seine eigene Facette behält.
    }

journal.window.stored.body.summary.stage.end.before.start = endete, bevor es begann

journal.window.stored.body.summary.stage.row.too.large = zu groß zum Behalten

journal.window.stored.body.summary.stage.span.too.long = deckt eine zu lange Zeitspanne ab

journal.window.stored.body.summary.stage.time = keine Zeit, die das Journal lesen konnte

journal.window.stored.body.summary.stage.type = keine Art, die das Journal lesen konnte

journal.window.stored.body.summary.strava.added.and.updated =     { $quantity ->
        [one] { $added }, 1 Training seit dem letzten Import geändert und auf den neuesten Stand gebracht.
       *[other] { $added }, { $updated } Trainings seit dem letzten Import geändert und auf den neuesten Stand gebracht.
    }

journal.window.stored.body.summary.strava.added.count =     { $quantity ->
        [one] 1 Training hinzugefügt
       *[other] { $count } Trainings hinzugefügt
    }

journal.window.stored.body.summary.strava.added.none = keine neuen Trainings.

journal.window.stored.body.summary.strava.added.only =     { $quantity ->
        [one] 1 Training hinzugefügt.
       *[other] { $count } Trainings hinzugefügt.
    }

journal.window.stored.body.summary.strava.conversion.moves.open =     { $quantity ->
        [one] 1 Datensatz konnte das Verschieben noch nicht abschließen. dein Journal versucht es beim nächsten Start erneut, und danach kannst du ihn bearbeiten.
       *[other] { $count } Datensätze konnten das Verschieben noch nicht abschließen. dein Journal versucht es beim nächsten Start erneut, und danach kannst du sie bearbeiten.
    }

journal.window.stored.body.summary.strava.conversion.no.workout =     { $quantity ->
        [one] 1 früherer Datensatz blieb unverändert, weil dein Journal nicht bestätigen konnte, zu welchem Training er gehört.
       *[other] { $count } frühere Datensätze blieben unverändert, weil dein Journal nicht bestätigen konnte, zu welchen Trainings sie gehören.
    }

journal.window.stored.body.summary.strava.conversion.waiting = einige frühere Trainings konnten noch nicht aktualisiert werden. dein Journal versucht es beim nächsten Start erneut.

journal.window.stored.body.summary.strava.facet.chosen = du hast die Facette { $facet } für diesen Import gewählt.

journal.window.stored.body.summary.strava.facet.chosen.none = du hast keine Facette für diesen Import gewählt: ein Training ohne Facette wird nicht auf der Seite Aktivitäten aufgelistet, es kommen keine Personen oder Dinge daraus, und Agenten, die auf einige Facetten beschränkt sind, sehen es nicht. die Suche und die Körper-App zeigen es weiterhin.

journal.window.stored.body.summary.strava.facet.default = neue Trainings werden unter der Facette { $facet } abgelegt, die du für Strava-Importe gewählt hast.

journal.window.stored.body.summary.strava.facet.unset = neue Trainings haben keine Facette, weil für diesen Import keine gewählt wurde und für Strava-Importe keine gesetzt ist, oder die gesetzte stummgeschaltet oder weg ist: ein Training ohne Facette wird nicht auf der Seite Aktivitäten aufgelistet, es kommen keine Personen oder Dinge daraus, und Agenten, die auf einige Facetten beschränkt sind, sehen es nicht. die Suche und die Körper-App zeigen es weiterhin.

journal.window.stored.body.summary.strava.filed =     { $quantity ->
        [one] 1 Training ist jetzt unter { $facet } abgelegt.
       *[other] { $count } Trainings sind jetzt unter { $facet } abgelegt.
    }

journal.window.stored.body.summary.strava.filed.one.moved = 1 Training ist jetzt unter { $facet } abgelegt. es wurde aus einer anderen Facette verschoben.

journal.window.stored.body.summary.strava.filed.some.moved =     { $quantity ->
        [one] 1 Training ist jetzt unter { $facet } abgelegt. { $moved } davon wurden aus einer anderen Facette verschoben.
       *[other] { $count } Trainings sind jetzt unter { $facet } abgelegt. { $moved } davon wurden aus einer anderen Facette verschoben.
    }

journal.window.stored.body.summary.strava.kept.filed =     { $quantity ->
        [one] 1 Training blieb unter { $facet }, wo es schon abgelegt war.
       *[other] { $count } Trainings blieben unter { $facet }, wo sie schon abgelegt waren.
    }

journal.window.stored.body.summary.strava.kept.sensed =     { $quantity ->
        [one] 1 Training wurde dort gelassen, wo es war, weil es schon auf der Seite Aktivitäten aufgelistet ist.
       *[other] { $count } Trainings wurden dort gelassen, wo sie waren, weil sie schon auf der Seite Aktivitäten aufgelistet sind.
    }

journal.window.stored.body.summary.strava.kept.unreadable =     { $quantity ->
        [one] 1 Training wurde dort gelassen, wo es war, weil dein Journal nicht lesen konnte, was an seinem Tag auf der Seite Aktivitäten aufgelistet ist. ein erneuter Import versucht es noch einmal.
       *[other] { $count } Trainings wurden dort gelassen, wo sie waren, weil dein Journal nicht lesen konnte, was an ihrem Tag auf der Seite Aktivitäten aufgelistet ist. ein erneuter Import versucht es noch einmal.
    }

journal.window.stored.body.summary.strava.listed.moved =     { $quantity ->
        [one] 1 Training wurde zur Facette dieses Imports verschoben, von { $facets }.
       *[other] { $count } Trainings wurden zur Facette dieses Imports verschoben, von { $facets }.
    }

journal.window.stored.body.summary.strava.listed.over.none.shown =     { $quantity ->
        [one] nichts, was schon abgelegt war, wurde verschoben: 1 Training wäre verschoben worden, und dir wurde keines angezeigt.
       *[other] nichts, was schon abgelegt war, wurde verschoben: { $count } Trainings wären verschoben worden, und dir wurde keines angezeigt.
    }

journal.window.stored.body.summary.strava.listed.over.shown =     { $quantity ->
        [one] nichts, was schon abgelegt war, wurde verschoben: 1 Training wäre verschoben worden, und dir wurde { $shown } mitgeteilt.
       *[other] nichts, was schon abgelegt war, wurde verschoben: { $count } Trainings wären verschoben worden, und dir wurde { $shown } mitgeteilt.
    }

journal.window.stored.body.summary.strava.names.cached =     { $quantity ->
        [one] die Wörter von 1 Training wurden bei einem früheren Import gelesen.
       *[other] die Wörter von { $count } Trainings wurden bei einem früheren Import gelesen.
    }

journal.window.stored.body.summary.strava.names.created =     { $quantity ->
        [one] 1 Person oder Ding wurde aus deinen Trainings hinzugefügt.
       *[other] { $count } Personen und Dinge wurden aus deinen Trainings hinzugefügt.
    }

journal.window.stored.body.summary.strava.names.elsewhere =     { $quantity ->
        [one] die Wörter von 1 Training wurden nicht gelesen, weil ein anderer Import gerade die Wörter von Trainings liest. sie werden gelesen, wenn du erneut importierst.
       *[other] die Wörter von { $count } Trainings wurden nicht gelesen, weil ein anderer Import gerade die Wörter von Trainings liest. sie werden gelesen, wenn du erneut importierst.
    }

journal.window.stored.body.summary.strava.names.failed =     { $quantity ->
        [one] 1 Training konnte nicht gelesen werden, und ein erneuter Import versucht es noch einmal.
       *[other] { $count } Trainings konnten nicht gelesen werden, und ein erneuter Import versucht es noch einmal.
    }

journal.window.stored.body.summary.strava.names.none.created = diesmal wurden keine Personen oder Dinge hinzugefügt.

journal.window.stored.body.summary.strava.names.promotion.unavailable = diesmal konnten keine Personen oder Dinge hinzugefügt werden, weil dein Journal deine Facetten nicht lesen konnte. ein erneuter Import versucht es noch einmal.

journal.window.stored.body.summary.strava.names.read =     { $quantity ->
        [one] die eigenen Wörter von 1 Training wurden für die Personen und die Ausrüstung gelesen, die sie nennen.
       *[other] die eigenen Wörter von { $count } Trainings wurden für die Personen und die Ausrüstung gelesen, die sie nennen.
    }

journal.window.stored.body.summary.strava.names.set.aside =     { $quantity ->
        [one] die Wörter von 1 Training wurden nicht gelesen, weil es unter einer Facette abgelegt ist, die stummgeschaltet oder weg ist. sie werden gelesen, sobald diese Facette wieder an ist oder ein Import sie unter einer anderen ablegt.
       *[other] die Wörter von { $count } Trainings wurden nicht gelesen, weil sie unter einer Facette abgelegt sind, die stummgeschaltet oder weg ist. sie werden gelesen, sobald diese Facette wieder an ist oder ein Import sie unter einer anderen ablegt.
    }

journal.window.stored.body.summary.strava.names.store.unavailable = dein Journal konnte nicht öffnen oder speichern, was es aus deinen Trainings notiert hat, also wurde diesmal nicht jede Person oder jedes Stück Ausrüstung darin gelesen oder hinzugefügt. ein erneuter Import versucht es noch einmal.

journal.window.stored.body.summary.strava.names.waiting.lane =     { $quantity ->
        [one] die Wörter von 1 Training wurden diesmal nicht gelesen: es ist kein Denken eingerichtet, das sie lesen darf, also werden sie gelesen, sobald es eingerichtet ist und du erneut importierst.
       *[other] die Wörter von { $count } Trainings wurden diesmal nicht gelesen: es ist kein Denken eingerichtet, das sie lesen darf, also werden sie gelesen, sobald es eingerichtet ist und du erneut importierst.
    }

journal.window.stored.body.summary.strava.names.waiting.no.condition =     { $quantity ->
        [one] die Wörter von 1 Training wurden diesmal nicht gelesen: dein Journal wartet auf ein Verarbeitungsfenster, und keines ist eingerichtet, also werden sie gelesen, sobald eines eingerichtet ist und du erneut importierst.
       *[other] die Wörter von { $count } Trainings wurden diesmal nicht gelesen: dein Journal wartet auf ein Verarbeitungsfenster, und keines ist eingerichtet, also werden sie gelesen, sobald eines eingerichtet ist und du erneut importierst.
    }

journal.window.stored.body.summary.strava.names.waiting.settings =     { $quantity ->
        [one] die Wörter von 1 Training wurden diesmal nicht gelesen: die Einstellungen deines Journals konnten nicht gelesen werden, also werden sie gelesen, wenn du erneut importierst.
       *[other] die Wörter von { $count } Trainings wurden diesmal nicht gelesen: die Einstellungen deines Journals konnten nicht gelesen werden, also werden sie gelesen, wenn du erneut importierst.
    }

journal.window.stored.body.summary.strava.names.waiting.stopped =     { $quantity ->
        [one] die Wörter von 1 Training wurden diesmal nicht gelesen: sie werden gelesen, wenn du erneut importierst.
       *[other] die Wörter von { $count } Trainings wurden diesmal nicht gelesen: sie werden gelesen, wenn du erneut importierst.
    }

journal.window.stored.body.summary.strava.names.waiting.window =     { $quantity ->
        [one] die Wörter von 1 Training wurden diesmal nicht gelesen: sie werden gelesen, wenn du während deines Verarbeitungsfensters importierst.
       *[other] die Wörter von { $count } Trainings wurden diesmal nicht gelesen: sie werden gelesen, wenn du während deines Verarbeitungsfensters importierst.
    }

journal.window.stored.body.summary.strava.records.no.facet =     { $quantity ->
        [one] 1 Training hat keine Facette, also wird es nicht auf der Seite Aktivitäten aufgelistet.
       *[other] { $count } Trainings haben keine Facette, also werden sie nicht auf der Seite Aktivitäten aufgelistet.
    }

journal.window.stored.body.summary.strava.records.refused = nicht jedes Training konnte diesmal auf der Seite Aktivitäten aufgelistet werden. ein erneuter Import versucht es noch einmal.

journal.window.stored.body.summary.strava.records.written =     { $quantity ->
        [one] 1 Training ist neu auf der Seite Aktivitäten für seinen Tag aufgelistet.
       *[other] { $count } Trainings sind neu auf der Seite Aktivitäten für ihre Tage aufgelistet.
    }

journal.window.stored.body.summary.strava.replaced =     { $quantity ->
        [one] 1 Training wurde erneut zur richtigen Zeit platziert, wo eine frühere Version deines Journals es zur falschen Zeit gesetzt hatte.
       *[other] { $count } Trainings wurden erneut zur richtigen Zeit platziert, wo eine frühere Version deines Journals sie zur falschen Zeit gesetzt hatte.
    }

journal.window.stored.body.summary.strava.section.activities = ## Aktivitäten

journal.window.stored.body.summary.strava.section.facet = ## Facette

journal.window.stored.body.summary.strava.section.people.and.gear = ## Personen und Ausrüstung

journal.window.stored.body.summary.strava.updated.only =     { $quantity ->
        [one] keine neuen Trainings, 1 Training seit dem letzten Import geändert und auf den neuesten Stand gebracht.
       *[other] keine neuen Trainings, { $updated } Trainings seit dem letzten Import geändert und auf den neuesten Stand gebracht.
    }

journal.window.stored.body.summary.took.facet =     { $quantity ->
        [one] 1 Teil deines Journals hat die Facette des Eintrags übernommen, den Oura in ihn verschoben hat, sodass das, was darin ist, unter dieser Facette lesbar ist.
       *[other] { $count } Teile deines Journals haben jeweils die Facette des Eintrags übernommen, den Oura in sie verschoben hat, sodass das, was darin ist, unter dieser Facette lesbar ist.
    }

journal.window.stored.body.summary.unsure.bullet =     { $quantity ->
        [one] - { $endpoint }: 1 nicht hinzugefügt; das Journal kann nicht erkennen, ob es in dem war, was du für { $days } gelöscht hast
       *[other] - { $endpoint }: { $count } nicht hinzugefügt; das Journal kann nicht erkennen, ob sie in dem waren, was du für { $days } gelöscht hast
    }

journal.window.stored.body.summary.windows.count =     { $quantity ->
        [one] 1 Teil deines Journals
       *[other] { $count } Teile deines Journals
    }

journal.window.stored.document.calls.count =     { $quantity ->
        [one] 1 Modellaufruf
       *[other] { $count } Modellaufrufe
    }

journal.window.stored.document.date = **Datum:** { $date } ({ $source })

journal.window.stored.document.engine.unknown = unbekannt

journal.window.stored.document.extraction = **Extraktion:** { $engine } — { $text_layer } Textebene, { $model_extracted } vom Modell extrahiert, { $unavailable } nicht verfügbar von { $pages }; { $image_described } mit Bildbeschreibung; { $model_calls }

journal.window.stored.document.marker.image.description = > [Bildbeschreibung — vom Modell erzeugt]

journal.window.stored.document.marker.image.description.raster = > [Bildbeschreibung — vom Modell erzeugt; Original: { $path }]

journal.window.stored.document.marker.image.description.unavailable.no.raster = > [Bildbeschreibung nicht verfügbar — { $reason }; es konnte kein Seitenbild erzeugt werden]

journal.window.stored.document.marker.image.description.unavailable.raster = > [Bildbeschreibung nicht verfügbar — { $reason }; Seitenbild bewahrt unter { $path }]

journal.window.stored.document.marker.image.description.unavailable.stripped = > [Bildbeschreibung nicht verfügbar — { $reason }]

journal.window.stored.document.marker.model.extracted = > [vom Modell aus dem Seitenbild extrahiert — kann Fehler enthalten]

journal.window.stored.document.marker.model.extracted.raster = > [vom Modell aus dem Seitenbild extrahiert — kann Fehler enthalten; Original: { $path }]

journal.window.stored.document.marker.page.text.unavailable.no.raster = > [Seitentext nicht verfügbar — { $reason }; es konnte kein Seitenbild erzeugt werden]

journal.window.stored.document.marker.page.text.unavailable.raster = > [Seitentext nicht verfügbar — { $reason }; Seitenbild bewahrt unter { $path }]

journal.window.stored.document.marker.page.text.unavailable.stripped = > [Seitentext nicht verfügbar — { $reason }]

journal.window.stored.document.page.heading = ## Seite { $page }

journal.window.stored.document.pages = **Seiten:** { $count }

journal.window.stored.document.pages.count =     { $quantity ->
        [one] 1 Seite
       *[other] { $count } Seiten
    }

journal.window.stored.document.type = **Typ:** Dokument

journal.window.stored.document.worker.warnings = **Worker-Warnungen:**

journal.window.stored.event.all.day.multi =     { $quantity ->
        [one] **{ $date }** ({ $days } Tag)
       *[other] **{ $date }** ({ $days } Tage)
    }

journal.window.stored.event.all.day.single = **{ $date }** (ganztägig)

journal.window.stored.event.preview.all.day = ganztägig

journal.window.stored.event.preview.days =     { $quantity ->
        [one] { $days } Tag
       *[other] { $days } Tage
    }

journal.window.stored.event.repeats = Wiederholt sich: { $rule }

journal.window.stored.event.rrule.count =     { $quantity ->
        [one] { $base }, 1 Mal
       *[other] { $base }, { $count } Mal
    }

journal.window.stored.event.rrule.daily = täglich

journal.window.stored.event.rrule.every.days = Alle { $interval } Tage

journal.window.stored.event.rrule.every.months = Alle { $interval } Monate

journal.window.stored.event.rrule.every.weeks = Alle { $interval } Wochen

journal.window.stored.event.rrule.every.years = Alle { $interval } Jahre

journal.window.stored.event.rrule.in.months = { $base } in { $months }

journal.window.stored.event.rrule.month.apr = Apr

journal.window.stored.event.rrule.month.aug = Aug

journal.window.stored.event.rrule.month.dec = Dez

journal.window.stored.event.rrule.month.feb = Feb

journal.window.stored.event.rrule.month.jan = Jan

journal.window.stored.event.rrule.month.jul = Jul

journal.window.stored.event.rrule.month.jun = Jun

journal.window.stored.event.rrule.month.mar = Mär

journal.window.stored.event.rrule.month.may = Mai

journal.window.stored.event.rrule.month.nov = Nov

journal.window.stored.event.rrule.month.oct = Okt

journal.window.stored.event.rrule.month.sep = Sep

journal.window.stored.event.rrule.monthly = Monatlich

journal.window.stored.event.rrule.on.days = { $base } an { $days }

journal.window.stored.event.rrule.on.monthday = { $base } am Tag { $days }

journal.window.stored.event.rrule.until = { $base }, bis { $until }

journal.window.stored.event.rrule.weekday.fri = Fr

journal.window.stored.event.rrule.weekday.mon = Mo

journal.window.stored.event.rrule.weekday.sat = Sa

journal.window.stored.event.rrule.weekday.sun = So

journal.window.stored.event.rrule.weekday.thu = Do

journal.window.stored.event.rrule.weekday.tue = Di

journal.window.stored.event.rrule.weekday.wed = Mi

journal.window.stored.event.rrule.weekly = wöchentlich

journal.window.stored.event.rrule.yearly = Jährlich

journal.window.stored.event.timed.duration = **{ $when }** ({ $minutes } min)

journal.window.stored.event.where = Wo: { $location }

journal.window.stored.event.with = Mit: { $names }

journal.window.stored.evernote.attachment.missing = ein Anhang fehlt in diesem Export.

journal.window.stored.evernote.attachment.oversized = ein Anhang ist über 25 MB, also wurde er ausgelassen.

journal.window.stored.evernote.attachment.unmatched = ein Anhang passte nicht zu dieser Notiz, also wurde er ausgelassen.

journal.window.stored.evernote.author = Autor: { $author }

journal.window.stored.evernote.encrypted = ein Teil dieser Notiz ist in Evernote verschlüsselt. er wurde nicht hereingeholt.

journal.window.stored.evernote.tags = Schlagwörter: { $tags }

journal.window.stored.evernote.task.due = { $title }, fällig { $due }

journal.window.stored.evernote.url = URL: { $url }

journal.window.stored.image.caption = **Bildunterschrift:**

journal.window.stored.image.date = **Datum:** { $date }

journal.window.stored.image.description.header = > [Bildbeschreibung — vom Modell abgeleitet]

journal.window.stored.image.description.unavailable = > nicht verfügbar — { $reason }

journal.window.stored.image.dimensions = **Abmessungen:** { $width }×{ $height }

journal.window.stored.image.format = **Format:** { $format }

journal.window.stored.image.type = **Typ:** Bild
