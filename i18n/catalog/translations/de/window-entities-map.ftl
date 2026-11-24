journal.window.entities.accessible.label.dismiss = verwerfen

journal.window.entities.accessible.name.close = schließen

journal.window.entities.accessible.name.entity.folders = Entitätsordner

journal.window.entities.accessible.name.search.detected.entities = erkannte Entitäten suchen

journal.window.entities.accessible.name.search.entities = Entitäten suchen

journal.window.entities.accessible.name.search.entity.cards = Entitätskarten suchen

journal.window.entities.accessible.name.sort.entities = Entitäten sortieren

journal.window.entities.aliases.other.names = auch bekannt als: { $aliases }

journal.window.entities.button.detail.reattach = wieder anhängen

journal.window.entities.button.label.attach.to.facet = an Facette anhängen

journal.window.entities.button.label.cancel = abbrechen

journal.window.entities.button.label.detach = lösen

journal.window.entities.button.label.hide = ausblenden

journal.window.entities.button.label.next = weiter

journal.window.entities.button.label.previous = vorherige

journal.window.entities.button.label.save.changes = Änderungen speichern

journal.window.entities.column.heading.description = Beschreibung

journal.window.entities.column.heading.name = Name

journal.window.entities.connections.calendar.last.day = { $day }

journal.window.entities.connections.evidence.show.more = { $shown_quantity ->
    [one] { $shown } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } werden angezeigt. mehr anzeigen
   *[other] { $shown } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } werden angezeigt. mehr anzeigen
}

journal.window.entities.connections.row.attendance = { $total_quantity ->
    [one] 1 gemeinsames Ereignis · { $day }
   *[other] { $total } gemeinsame Ereignisse · { $day }
}

journal.window.entities.connections.row.direct = { $total_quantity ->
    [one] 1 Moment · { $day }
   *[other] { $total } Momente · { $day }
}

journal.window.entities.connections.summary = { $direct_quantity ->
    [one] { $direct } mit direktem Nachweis · { $shared_quantity ->
        [one] { $shared }
       *[other] { $shared }
    } nur aus gemeinsamen Ereignissen
   *[other] { $direct } mit direktem Nachweis · { $shared_quantity ->
        [one] { $shared }
       *[other] { $shared }
    } nur aus gemeinsamen Ereignissen
}

journal.window.entities.connections.summary.with.calendar = { $direct_quantity ->
    [one] { $direct } mit direktem Nachweis · { $shared_quantity ->
        [one] { $shared }
       *[other] { $shared }
    } nur aus gemeinsamen Ereignissen · { $calendar_quantity ->
        [one] { $calendar }
       *[other] { $calendar }
    } nur aus deinem Kalender
   *[other] { $direct } mit direktem Nachweis · { $shared_quantity ->
        [one] { $shared }
       *[other] { $shared }
    } nur aus gemeinsamen Ereignissen · { $calendar_quantity ->
        [one] { $calendar }
       *[other] { $calendar }
    } nur aus deinem Kalender
}

journal.window.entities.connections.you.calendar.latest = nur in deinem Kalender · zuletzt { $day }

journal.window.entities.connections.you.latest.attended.with = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: Ereignisse ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: Ereignisse ({ $day })
}

journal.window.entities.connections.you.latest.co.present = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: nahe beieinander ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: nahe beieinander ({ $day })
}

journal.window.entities.connections.you.latest.created = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: erstellt ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: erstellt ({ $day })
}

journal.window.entities.connections.you.latest.fallback = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: { $evidence_name } ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: { $evidence_name } ({ $day })
}

journal.window.entities.connections.you.latest.family.of = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: Familie ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: Familie ({ $day })
}

journal.window.entities.connections.you.latest.knows = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: kennt ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: kennt ({ $day })
}

journal.window.entities.connections.you.latest.mentioned = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: erwähnt ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: erwähnt ({ $day })
}

journal.window.entities.connections.you.latest.messaged.with = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: Nachricht ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: Nachricht ({ $day })
}

journal.window.entities.connections.you.latest.other = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: zugehörig ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: zugehörig ({ $day })
}

journal.window.entities.connections.you.latest.party.of = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: beteiligt an ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: beteiligt an ({ $day })
}

journal.window.entities.connections.you.latest.reports.to = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: berichtet an ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: berichtet an ({ $day })
}

journal.window.entities.connections.you.latest.scheduled.with = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: geplant ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: geplant ({ $day })
}

journal.window.entities.connections.you.latest.spoke.with = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: gesprochen ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: gesprochen ({ $day })
}

journal.window.entities.connections.you.latest.uses = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: nutzt ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: nutzt ({ $day })
}

journal.window.entities.connections.you.latest.works.at = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: arbeitet bei ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: arbeitet bei ({ $day })
}

journal.window.entities.connections.you.latest.works.with = { $moments_quantity ->
    [one] 1 gemeinsamer Moment · zuletzt: arbeitet mit ({ $day })
   *[other] { $moments } gemeinsame Momente · zuletzt: arbeitet mit ({ $day })
}

journal.window.entities.copy.cards.empty = noch keine Entitäten in dieser Facette. markiere unten mit einem Stern, was du behalten willst, oder füge selbst eine hinzu.

journal.window.entities.copy.conn.ambiguous = das stimmt vielleicht mit mehr als einer Entität überein.

journal.window.entities.copy.conn.calendar.only = nur Kalender

journal.window.entities.copy.conn.empty = noch keine Verbindungen gespeichert. sie erscheinen, wenn diese Entität mit jemand anderem oder etwas anderem auftaucht.

journal.window.entities.copy.conn.events.only = nur Ereignisse

journal.window.entities.copy.conn.evidence.caption = die Momente dahinter: Nachweis, keine Schlussfolgerung

journal.window.entities.copy.conn.evidence.failed = diese Momente konnten nicht geladen werden. versuch es noch einmal.

journal.window.entities.copy.conn.evidence.more = zeigt { $slot1 } von { $slot2 }. mehr anzeigen

journal.window.entities.copy.conn.evidence.view.entity = Entität anzeigen →

journal.window.entities.copy.conn.horizon = Verbindungen von Zeit am selben Ort reichen nur bis { $slot1 } zurück. die { $slot2 } früheren Tage sind noch in deinem Journal; diese Verbindungen sind noch nicht aufgebaut.

journal.window.entities.copy.conn.horizon.one = Verbindungen von Zeit am selben Ort reichen nur bis { $slot1 } zurück. ein früherer Tag ist noch in deinem Journal; diese Verbindungen sind noch nicht aufgebaut.

journal.window.entities.copy.conn.index.action = Systemzustand prüfen →

journal.window.entities.copy.conn.index.unavailable = Verbindungen sind gerade nicht verfügbar.

journal.window.entities.copy.conn.kind.chip.words.mentioned = Erwähnungen

journal.window.entities.copy.conn.kind.words.attended.with = Ereignisse

journal.window.entities.copy.conn.kind.words.co.present = beieinander

journal.window.entities.copy.conn.kind.words.created = erstellt

journal.window.entities.copy.conn.kind.words.family.of = Familie

journal.window.entities.copy.conn.kind.words.knows = kennt

journal.window.entities.copy.conn.kind.words.mentioned = erwähnt

journal.window.entities.copy.conn.kind.words.messaged.with = per Nachricht

journal.window.entities.copy.conn.kind.words.other = in Beziehung

journal.window.entities.copy.conn.kind.words.party.of = beteiligt

journal.window.entities.copy.conn.kind.words.reports.to = berichtet an

journal.window.entities.copy.conn.kind.words.scheduled.with = geplant

journal.window.entities.copy.conn.kind.words.spoke.with = gesprochen

journal.window.entities.copy.conn.kind.words.uses = nutzt

journal.window.entities.copy.conn.kind.words.works.at = arbeitet bei

journal.window.entities.copy.conn.kind.words.works.with = arbeitet mit

journal.window.entities.copy.conn.label.fallbacks.co.present = Zeit am selben Ort

journal.window.entities.copy.conn.label.fallbacks.mentioned = kam im Gespräch vor

journal.window.entities.copy.conn.label.fallbacks.spoke.with = ein Gespräch

journal.window.entities.copy.conn.load.failed = Verbindungen konnten nicht geladen werden. versuch es noch einmal.

journal.window.entities.copy.conn.row.meta = { $slot1 } Momente · { $slot2 }

journal.window.entities.copy.conn.row.meta.attendance = { $slot1 } Ereignisse zusammen · { $slot2 }

journal.window.entities.copy.conn.row.meta.one = 1 Moment · { $slot1 }

journal.window.entities.copy.conn.row.meta.one.attendance = 1 Ereignis zusammen · { $slot1 }

journal.window.entities.copy.conn.row.meta.scheduled = { $slot1 }

journal.window.entities.copy.conn.source.word.fallback = was dein Journal hält

journal.window.entities.copy.conn.source.words.calendar = Kalenderereignisse

journal.window.entities.copy.conn.source.words.co.presence = gemeinsame Momente

journal.window.entities.copy.conn.source.words.document = Dokumente

journal.window.entities.copy.conn.source.words.event.legacy = ältere Kalenderereignisse

journal.window.entities.copy.conn.source.words.mention = Gespräch

journal.window.entities.copy.conn.source.words.messaging = Nachrichten

journal.window.entities.copy.conn.source.words.observation = was dein Journal hält

journal.window.entities.copy.conn.source.words.participation = Aktivitätsdatensätze

journal.window.entities.copy.conn.source.words.relation = was dein Journal hält

journal.window.entities.copy.conn.source.words.speaker = Gespräch

journal.window.entities.copy.conn.summary = { $slot1 } mit direktem Nachweis · { $slot2 } nur aus gemeinsamen Ereignissen

journal.window.entities.copy.conn.summary.calendar = { $slot1 } nur aus deinem Kalender

journal.window.entities.copy.conn.with.you.eyebrow = mit dir

journal.window.entities.copy.conn.with.you.failed = dein eigener Verlauf mit dieser Entität konnte gerade eben nicht geprüft werden.

journal.window.entities.copy.conn.with.you.summary = { $slot1 } Momente zusammen · zuletzt { $slot2 } ({ $slot3 })

journal.window.entities.copy.conn.with.you.summary.one = 1 Moment zusammen · zuletzt { $slot1 } ({ $slot2 })

journal.window.entities.copy.conn.with.you.summary.scheduled = nur in deinem Kalender · zuletzt { $slot1 }

journal.window.entities.copy.conn.with.you.toggle = die Momente dahinter ›

journal.window.entities.copy.detach.confirm = { $slot1 } von { $slot2 } lösen? bleibt in deinem Journal und du kannst jederzeit wieder anhängen.

journal.window.entities.copy.detach.done = von { $slot1 } gelöst. { $slot2 } ist noch in deinem Journal.

journal.window.entities.copy.detach.find.action = in deinem Journal finden →

journal.window.entities.copy.detach.reattach.action = wieder anhängen →

journal.window.entities.copy.detected.all.hidden = alles, was hier in letzter Zeit erkannt wurde, ist ausgeblendet.

journal.window.entities.copy.detected.empty = hier ist noch nichts. wenn du etwas zu deinem Journal hinzufügst, erscheinen hier Personen, Orte und Dinge.

journal.window.entities.copy.detected.hidden.badge = ausgeblendet

journal.window.entities.copy.detected.hidden.count = { $slot1 } ausgeblendete Namen in { $slot2 }

journal.window.entities.copy.detected.hidden.count.one = 1 ausgeblendeter Name in { $slot1 }

journal.window.entities.copy.detected.hidden.heading = ausgeblendet in { $slot1 }

journal.window.entities.copy.detected.hidden.last = zuletzt erkannt { $slot1 }

journal.window.entities.copy.detected.hidden.lede = ein ausgeblendeter Name bleibt in deinem Journal und bleibt ausgeblendet, egal wie oft er erneut erkannt wird. seine Erkennungen hier sind aus der Suche ausgelassen, verbundene Agenten können sie nicht erreichen, und dein Journal fügt ihn dieser Facette nicht hinzu.

journal.window.entities.copy.detected.hidden.show = Ausgeblendete anzeigen

journal.window.entities.copy.detected.hidden.stop.show = Ausgeblendete nicht mehr anzeigen

journal.window.entities.copy.detected.hide.action = { $slot1 } in { $slot2 } ausblenden

journal.window.entities.copy.detected.hide.done = { $slot1 } ist in { $slot2 } ausgeblendet.

journal.window.entities.copy.detected.hide.done.detail = es bleibt ausgeblendet, egal wie oft es erneut erkannt wird.

journal.window.entities.copy.detected.hide.failed = { $slot1 } konnte nicht ausgeblendet werden. nichts hat sich geändert.

journal.window.entities.copy.detected.hide.no.search = die Suche und verbundene Agenten können deren Erkennungen hier noch finden.

journal.window.entities.copy.detected.hide.try.again = erneut versuchen

journal.window.entities.copy.detected.still.hidden = { $slot1 } stimmt noch mit { $slot2 } überein, einer ausgeblendeten Entität.

journal.window.entities.copy.detected.still.hidden.open = { $slot1 } öffnen

journal.window.entities.copy.detected.unhide = wieder einblenden

journal.window.entities.copy.detected.unhide.failed = { $slot1 } konnte nicht wieder eingeblendet werden. nichts hat sich geändert.

journal.window.entities.copy.detected.unreadable = Erkennungen können hier nicht angezeigt werden. dein Journal konnte nicht lesen, welche Namen du in dieser Facette ausgeblendet hast.

journal.window.entities.copy.faces.busy = dein Journal ist gerade mit Gesichtsdaten beschäftigt, deshalb hat sich nichts geändert.

journal.window.entities.copy.faces.change = Profilbild ändern

journal.window.entities.copy.faces.changed = das hat sich seit dem Laden der Seite geändert, deshalb wurde es nicht rückgängig gemacht.

journal.window.entities.copy.faces.chosen.alt = Profilbild von { $slot1 }

journal.window.entities.copy.faces.confirmed.lede = du hast dieses Profilbild für { $slot1 } gewählt.

journal.window.entities.copy.faces.disabled = Profilbildvorschläge sind aus, deshalb hat sich nichts geändert.

journal.window.entities.copy.faces.dismissed.lede = du hast kein Profilbild für { $slot1 } gewählt. wenn weitere Gesichter auftauchen, die { $slot2 } sein könnten, können sie hier erscheinen.

journal.window.entities.copy.faces.done.chosen = Profilbild für { $slot1 } gespeichert.

journal.window.entities.copy.faces.done.dismissed = kein Profilbild für { $slot1 }.

journal.window.entities.copy.faces.done.rejected = ok, das ist nicht { $slot1 }.

journal.window.entities.copy.faces.face.alt = ein Gesicht aus einer Besprechung am { $slot1 }, das { $slot2 } sein könnte

journal.window.entities.copy.faces.facet.hint = Profilbild für { $slot1 } wählen

journal.window.entities.copy.faces.failed = das wurde nicht gespeichert.

journal.window.entities.copy.faces.from.day = aus einer Besprechung am { $slot1 }

journal.window.entities.copy.faces.gone = dieses Gesicht ist nicht mehr verfügbar.

journal.window.entities.copy.faces.heading = Profilbild

journal.window.entities.copy.faces.load.failed = diese Gesichter konnten nicht geladen werden.

journal.window.entities.copy.faces.merge.reason = Gesichter aus deinen Besprechungen legen nahe, dass sie dieselbe Person sein könnten.

journal.window.entities.copy.faces.none = nichts davon

journal.window.entities.copy.faces.not.them = nicht { $slot1 }

journal.window.entities.copy.faces.possible.lede = diese Gesichter aus deinen Besprechungen könnten { $slot1 } sein. keins wird verwendet, bis du eins wählst.

journal.window.entities.copy.faces.possible.title = Profilbild für { $slot1 } wählen

journal.window.entities.copy.faces.refresh = aktualisieren

journal.window.entities.copy.faces.remove = kein Profilbild verwenden

journal.window.entities.copy.faces.retry = erneut versuchen

journal.window.entities.copy.faces.see.others = anderes wählen

journal.window.entities.copy.faces.settings.link = schalte sie in den Einstellungen ein

journal.window.entities.copy.faces.show.again = rückgängig machen

journal.window.entities.copy.faces.source.anchored = neben dem Namen

journal.window.entities.copy.faces.source.box = in der Videokachel

journal.window.entities.copy.faces.stale = das hat sich seit dem Laden der Seite geändert, deshalb wurde nichts gespeichert.

journal.window.entities.copy.faces.suggested.badge = vorgeschlagen

journal.window.entities.copy.faces.suggested.lede = dein Journal hat dieses Gesicht aus deinen Besprechungsaufnahmen gewählt. es ist nur ein Vorschlag, bis du ja sagst.

journal.window.entities.copy.faces.suggested.title = ist das { $slot1 }?

journal.window.entities.copy.faces.unconfirmed.alt = ein Gesicht aus einer Besprechung, das { $slot1 } sein könnte, nicht bestätigt

journal.window.entities.copy.faces.unconfirmed.badge = nicht bestätigt

journal.window.entities.copy.faces.undo = rückgängig machen

journal.window.entities.copy.faces.undone = rückgängig gemacht.

journal.window.entities.copy.faces.use = dieses Profilbild verwenden

journal.window.entities.copy.faces.yes = ja, dieses Profilbild verwenden

journal.window.entities.copy.grid.clear.range = Zeitraum leeren

journal.window.entities.copy.grid.day.one = Tag

journal.window.entities.copy.grid.day.other = Tage

journal.window.entities.copy.grid.default.day = neuester Tag: { $slot1 }

journal.window.entities.copy.grid.lede = { $slot1 } { $slot2 } über { $slot3 } { $slot4 }

journal.window.entities.copy.grid.range = { $slot1 } – { $slot2 }

journal.window.entities.copy.grid.show.all = alle { $slot1 } { $slot2 } anzeigen

journal.window.entities.copy.grid.title = Momente im Zeitverlauf

journal.window.entities.copy.grid.unit.none = noch nichts

journal.window.entities.copy.grid.unit.one = Moment

journal.window.entities.copy.grid.unit.other = Momente

journal.window.entities.copy.hide.pane.help = verschiebt es zu den Ausgeblendeten, wo du es wieder einblenden kannst. die Suche lässt seine Notizen und seine Erkennungen in seinen Facetten aus, und verbundene Agenten können sie nicht erreichen.

journal.window.entities.copy.obs.disclosure = zeigt die { $slot1 } neuesten von { $slot2 } Momenten

journal.window.entities.copy.obs.empty = noch nichts mit { $slot1 }.

journal.window.entities.copy.obs.heading = Momente

journal.window.entities.copy.obs.load.failed = diese Momente konnten nicht geladen werden. versuch es noch einmal →

journal.window.entities.copy.obs.source.link.title = diesen Tag ansehen

journal.window.entities.copy.scope.empty.action = dein ganzes Journal anzeigen

journal.window.entities.copy.scope.empty.body = Entitäten erscheinen hier, sobald dein Journal in dieser Facette Personen, Orte und Projekte hält.

journal.window.entities.copy.scope.empty.title = noch nichts in { $slot1 }.

journal.window.entities.copy.scope.facet.missing = diese Facette ist nicht mehr in deinem Journal, also ist das dein ganzes Journal.

journal.window.entities.copy.scope.showing = zeigt

journal.window.entities.copy.scope.whole.journal = dein ganzes Journal

journal.window.entities.copy.trust.action.failed = diese Änderung der Entität wurde nicht abgeschlossen.

journal.window.entities.copy.trust.heading = Identitätsvertrauen

journal.window.entities.copy.trust.history.change = { $slot1 } → { $slot2 }

journal.window.entities.copy.trust.history.empty = noch kein Identitätsverlauf verfügbar.

journal.window.entities.copy.trust.history.heading = Versionsverlauf

journal.window.entities.copy.trust.history.suggested.by = vorgeschlagen von { $slot1 }

journal.window.entities.copy.trust.history.summary = { $slot1 } · { $slot2 } · { $slot3 }

journal.window.entities.copy.trust.identity.none = keine frühere Identität

journal.window.entities.copy.trust.identity.summary = { $slot1 } · { $slot2 } · auch bekannt als: { $slot3 }

journal.window.entities.copy.trust.load.failed = die Details zum Identitätsvertrauen konnten nicht geladen werden. versuch es noch einmal.

journal.window.entities.copy.trust.merge.action = Zusammenführung als Vorschau anzeigen

journal.window.entities.copy.trust.merge.confirm = { $slot1 } in { $slot2 } zusammenführen? eine Zusammenführung lässt sich nicht rückgängig machen.

journal.window.entities.copy.trust.merge.confirm.action = Zusammenführen bestätigen

journal.window.entities.copy.trust.merge.done = zusammengeführt.

journal.window.entities.copy.trust.merge.label = zusammenführen in

journal.window.entities.copy.trust.preview.counts = Stimmproben hinzugefügt: { $slot1 } · Proben nach dem Zusammenführen: { $slot2 } · Notizen unter dem Namen, der zusammengeführt wird: { $slot3 } · Sprecherbeschriftungen geändert: { $slot4 } · Aktivitätsdatensätze geändert: { $slot5 }

journal.window.entities.copy.trust.preview.summary = Aliasse hinzugefügt: { $slot1 } · E-Mails hinzugefügt: { $slot2 }

journal.window.entities.copy.trust.restore.action = diese Version wiederherstellen

journal.window.entities.copy.trust.restore.confirm = die Identität aus dieser Version wiederherstellen? die aktuelle Identität bleibt im Verlauf.

journal.window.entities.copy.trust.suggestion.summary = { $slot1 } → { $slot2 } · { $slot3 }

journal.window.entities.copy.trust.suggestions.empty = keine offenen Zusammenführungsvorschläge betreffen diese Entität.

journal.window.entities.copy.trust.suggestions.heading = offene Zusammenführungsvorschläge

journal.window.entities.copy.voice.changed = diese Probe hat sich geändert. lade die Liste neu, bevor du sie entfernst.

journal.window.entities.copy.voice.empty = keine Stimmproben für diese Person gespeichert.

journal.window.entities.copy.voice.error = diese Stimmproben konnten nicht gelesen oder geändert werden. versuch es noch einmal.

journal.window.entities.copy.voice.heading = Stimmproben

journal.window.entities.copy.voice.loading = Stimmproben werden geladen…

journal.window.entities.copy.voice.more = mehr anzeigen

journal.window.entities.copy.voice.open = Aufnahme öffnen

journal.window.entities.copy.voice.remove = entfernen

journal.window.entities.copy.voice.retry = erneut versuchen

journal.window.entities.copy.voice.sentence = Satz { $slot1 }

journal.window.entities.delete.all.items.gone = { $removed_quantity ->
    [one] es ist hier nicht mehr aufgeführt.
   *[other] sie sind hier nicht mehr aufgeführt.
}

journal.window.entities.delete.couldn.t.erase.that = diese Entität konnte nicht endgültig gelöscht werden.

journal.window.entities.delete.couldn.t.recover.that = diese Entität konnte nicht wiederhergestellt werden.

journal.window.entities.delete.detail.action.help = { $action_start }löschen{ $action_end } { $description_start }verschiebt sie in den Papierkorb, wo du sie wiederherstellen kannst, bis sie endgültig gelöscht wird.{ $description_end }

journal.window.entities.delete.erased.from.your.journal = { $total_quantity ->
    [one] { $total } Entität aus deinem Journal endgültig gelöscht
   *[other] { $total } Entitäten aus deinem Journal endgültig gelöscht
}

journal.window.entities.delete.erased.of = { $processed_quantity ->
    [one] { $processed } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } endgültig gelöscht
   *[other] { $processed } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } endgültig gelöscht
}

journal.window.entities.delete.erases.unless.you.recover.it = wird { $erase_day } endgültig gelöscht, außer du stellst es wieder her.

journal.window.entities.delete.facet.many.its.place.and.notes.in = { $facets_quantity ->
    [one] ihr Ort und ihre Notizen in { $facets } Facette
   *[other] ihr Ort und ihre Notizen in { $facets } Facetten
}

journal.window.entities.delete.gone.more.many.more.are.no.longer.listed = { $gone_quantity ->
    [one] { $gone } weiteres ist nicht mehr aufgeführt.
   *[other] { $gone } weitere sind nicht mehr aufgeführt.
}

journal.window.entities.delete.items.find.after.failed.erase = { $remaining_quantity ->
        [one] such es unten oder im Papierkorb.
       *[other] such sie unten oder im Papierkorb.
    }

journal.window.entities.delete.many.delete.entities = { $total_quantity ->
    [one] { $total } Entität löschen?
   *[other] { $total } Entitäten löschen?
}

journal.window.entities.delete.many.erase.entities.now = { $total_quantity ->
    [one] { $total } Entität jetzt endgültig löschen?
   *[other] { $total } Entitäten jetzt endgültig löschen?
}

journal.window.entities.delete.marked.many.couldn.t.be.moved.and = { $remaining_quantity ->
    [one] { $remaining } konnte nicht verschoben werden und ist unten markiert.
   *[other] { $remaining } konnten nicht verschoben werden und sind unten markiert.
}

journal.window.entities.delete.more.and.more = { $more_quantity ->
    [one] und { $more } weiteres
   *[other] und { $more } weitere
}

journal.window.entities.delete.moved.of.to.the = { $processed_quantity ->
    [one] { $processed } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } in den Papierkorb verschoben
   *[other] { $processed } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } in den Papierkorb verschoben
}

journal.window.entities.delete.moved.to.the.trash = { $total_quantity ->
    [one] { $total } Entität in den Papierkorb verschoben
   *[other] { $total } Entitäten in den Papierkorb verschoben
}

journal.window.entities.delete.none.still.many.are.still.in.your.journal = { $remaining_quantity ->
        [one] { $remaining } ist noch in deinem Journal, unten markiert.
       *[other] { $remaining } sind noch in deinem Journal, unten markiert.
    }

journal.window.entities.delete.noun.erased.from.your.journal = Entität aus deinem Journal endgültig gelöscht

journal.window.entities.delete.other.items.gone = { $other_quantity ->
    [one] das andere ist hier nicht mehr aufgeführt.
   *[other] die anderen sind hier nicht mehr aufgeführt.
}

journal.window.entities.delete.partial.items.not.erased = { $remaining_quantity ->
        [one] das andere wurde nicht endgültig gelöscht. such es unten oder im Papierkorb.
       *[other] die anderen wurden nicht endgültig gelöscht. such sie unten oder im Papierkorb.
    }

journal.window.entities.delete.partial.items.not.recovered = { $remaining_quantity ->
        [one] das andere ist noch im Papierkorb. du kannst es von dort noch einmal versuchen.
       *[other] die anderen sind noch im Papierkorb. du kannst sie von dort noch einmal versuchen.
    }

journal.window.entities.delete.pick.label.select.name = { $entity_name } auswählen

journal.window.entities.delete.recovered.count = { $total_quantity ->
    [one] { $total } Entität wiederhergestellt
   *[other] { $total } Entitäten wiederhergestellt
}

journal.window.entities.delete.recovered.of = { $processed_quantity ->
    [one] { $processed } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } wiederhergestellt
   *[other] { $processed } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } wiederhergestellt
}

journal.window.entities.delete.recovered.one = Entität wiederhergestellt

journal.window.entities.delete.selected.selected = { $selected_quantity ->
    [one] { $selected } ausgewählt
   *[other] { $selected } ausgewählt
}

journal.window.entities.delete.still.many.couldn.t.be.moved.and = { $remaining_quantity ->
        [one] { $remaining } konnte nicht verschoben werden und ist noch in deinem Journal, unten markiert.
       *[other] { $remaining } konnten nicht verschoben werden und sind noch in deinem Journal, unten markiert.
    }

journal.window.entities.delete.they.erase.unless.you.recover = sie werden { $erase_day } endgültig gelöscht, außer du stellst sie wieder her.

journal.window.entities.detail.aliases.other.names = Auch bekannt als: { $aliases }

journal.window.entities.detail.facets.count = { $facets_quantity ->
    [one] 1 Facette
   *[other] { $facets } Facetten
}

journal.window.entities.detail.id.description = dieser Name sieht aus wie eine ID.

journal.window.entities.detail.last.mentioned.date = Zuletzt erwähnt: { $day }

journal.window.entities.detail.last.mentioned.full.day = Zuletzt erwähnt: { $day }

journal.window.entities.detected.hidden.heading = ausgeblendet in { $facet_name }

journal.window.entities.detected.hidden.in.facet = { $hidden_quantity ->
    [one] 1 Name in { $facet_name } ausgeblendet
   *[other] { $hidden } Namen in { $facet_name } ausgeblendet
}

journal.window.entities.detected.hidden.last.detected = zuletzt erkannt { $day }

journal.window.entities.detected.hidden.reappeared = { $entity_name } stimmt noch mit { $matched_name } überein, einer ausgeblendeten Entität.

journal.window.entities.detected.hidden.reappeared.open = { $matched_name } öffnen

journal.window.entities.detected.hide.action = { $entity_name } in { $facet_name } ausblenden

journal.window.entities.detected.hide.done = { $entity_name } ist in { $facet_name } ausgeblendet.

journal.window.entities.detected.hide.failed = { $entity_name } konnte nicht ausgeblendet werden. nichts hat sich geändert.

journal.window.entities.detected.page.range = { $total_quantity ->
    [one] { $first }–{ $last } von { $total } erkannter Entität werden angezeigt
   *[other] { $first }–{ $last } von { $total } erkannten Entitäten werden angezeigt
}

journal.window.entities.detected.unhide.failed = { $entity_name } konnte nicht wieder eingeblendet werden. nichts hat sich geändert.

journal.window.entities.entity.delete.copy.again.if.your.journal.finds.it.again = wenn dein Journal sie später wieder findet, wird sie als neue Entität hinzugefügt.

journal.window.entities.entity.delete.copy.again.many.any.of.them.your.journal.finds = jede, die dein Journal später wieder findet, wird als neue Entität hinzugefügt.

journal.window.entities.entity.delete.copy.back.back = zurück

journal.window.entities.entity.delete.copy.busy.something.else.is.changing.it.right = etwas anderes ändert sie gerade. versuch es in einer Minute noch einmal.

journal.window.entities.entity.delete.copy.busy.something.else.is.changing.this.right = etwas anderes ändert das gerade, deshalb wurde es nicht verschoben. versuch es in einer Minute noch einmal.

journal.window.entities.entity.delete.copy.busy.something.else.is.changing.this.right.variant2 = etwas anderes ändert das gerade, deshalb wurde es nicht endgültig gelöscht. versuch es in einer Minute noch einmal.

journal.window.entities.entity.delete.copy.button.delete = löschen

journal.window.entities.entity.delete.copy.cancel.cancel = abbrechen

journal.window.entities.entity.delete.copy.delete.delete = löschen

journal.window.entities.entity.delete.copy.details.its.details.and.version.history = ihre Details und ihr Versionsverlauf

journal.window.entities.entity.delete.copy.done.done = fertig

journal.window.entities.entity.delete.copy.erase.now.erase.now = jetzt endgültig löschen

journal.window.entities.entity.delete.copy.facet.one.its.place.and.notes.in.facet = ihr Ort und ihre Notizen in 1 Facette

journal.window.entities.entity.delete.copy.facet.unknown.its.place.and.notes.in.each = ihr Ort und ihre Notizen in jeder Facette, in der sie ist

journal.window.entities.entity.delete.copy.gone.it.s.no.longer.in.your = sie ist nicht mehr in deinem Journal.

journal.window.entities.entity.delete.copy.last.facet.your.journal.needs.at.least.one = dein Journal braucht mindestens eine Facette, die nicht stummgeschaltet ist.

journal.window.entities.entity.delete.copy.lead.this.erases.the.entity.s.own = das löscht endgültig bei der Entität selbst:

journal.window.entities.entity.delete.copy.lead.this.moves.the.entity.s.own = das verschiebt bei der Entität selbst:

journal.window.entities.entity.delete.copy.move.to.trash.move.to.trash = in den Papierkorb verschieben

journal.window.entities.entity.delete.copy.open.trash.open.trash = Papierkorb öffnen

journal.window.entities.entity.delete.copy.other.this.wasn.t.erased.try.again = das wurde nicht endgültig gelöscht. versuch es noch einmal.

journal.window.entities.entity.delete.copy.other.this.wasn.t.moved.try.again = das wurde nicht verschoben. versuch es noch einmal.

journal.window.entities.entity.delete.copy.own.many.this.erases.each.one.s.own = das löscht endgültig von jeder einzelnen die eigenen Details, den Versionsverlauf, den Ort und die Notizen in jeder Facette und das, was dein Journal über ihre Stimme behält, falls überhaupt etwas.

journal.window.entities.entity.delete.copy.own.many.this.moves.each.one.s.own = das verschiebt von jeder einzelnen die eigenen Details, den Versionsverlauf, den Ort und die Notizen in jeder Facette und das, was dein Journal über ihre Stimme behält, falls überhaupt etwas.

journal.window.entities.entity.delete.copy.principal.this.is.you.so.it.can = das bist du, also kann es nicht gelöscht werden.

journal.window.entities.entity.delete.copy.reach.it.leaves.your.journal.now.and = sie verlässt dein Journal jetzt und kann nicht wiederhergestellt werden.

journal.window.entities.entity.delete.copy.reach.it.leaves.your.journal.now.and.variant2 = sie verlässt dein Journal jetzt und wartet im Papierkorb, danach wird sie endgültig gelöscht. bis dahin kannst du sie wiederherstellen.

journal.window.entities.entity.delete.copy.reach.many.they.leave.your.journal.now.and = sie verlassen dein Journal jetzt und können nicht wiederhergestellt werden.

journal.window.entities.entity.delete.copy.reach.many.they.leave.your.journal.now.and.variant2 = sie verlassen dein Journal jetzt und warten im Papierkorb, danach werden sie endgültig gelöscht. bis dahin kannst du jede einzelne wiederherstellen.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its = der Datensatz der Entitäten deines Journals konnte nicht aktualisiert werden. nichts wurde verschoben.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its.variant2 = der Datensatz der Entitäten deines Journals konnte nicht aktualisiert werden. nichts wurde endgültig gelöscht.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its.variant3 = der Datensatz der Entitäten deines Journals konnte nicht aktualisiert werden, also wurde das nicht verschoben.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its.variant4 = der Datensatz der Entitäten deines Journals konnte nicht aktualisiert werden, also wurde das nicht endgültig gelöscht.

journal.window.entities.entity.delete.copy.recover.recover = wiederherstellen

journal.window.entities.entity.delete.copy.select.all.page.select.all.on.this.page = alle auf dieser Seite auswählen

journal.window.entities.entity.delete.copy.select.all.select.all = alle auswählen

journal.window.entities.entity.delete.copy.select.select = auswählen

journal.window.entities.entity.delete.copy.stays.many.transcripts.summaries.and.activities.elsewhere.in = Transkripte, Zusammenfassungen und Aktivitäten anderswo in deinem Journal werden nicht endgültig gelöscht und können sie noch erwähnen.

journal.window.entities.entity.delete.copy.stays.many.transcripts.summaries.and.activities.elsewhere.in.variant2 = Transkripte, Zusammenfassungen und Aktivitäten anderswo in deinem Journal bleiben, wo sie sind, und können sie noch erwähnen.

journal.window.entities.entity.delete.copy.stays.transcripts.summaries.and.activities.elsewhere.in = Transkripte, Zusammenfassungen und Aktivitäten anderswo in deinem Journal werden nicht endgültig gelöscht und können sie noch erwähnen.

journal.window.entities.entity.delete.copy.stays.transcripts.summaries.and.activities.elsewhere.in.variant2 = Transkripte, Zusammenfassungen und Aktivitäten anderswo in deinem Journal bleiben, wo sie sind, und können sie noch erwähnen.

journal.window.entities.entity.delete.copy.voice.unknown.what.your.journal.keeps.about.its = was dein Journal über ihre Stimme behält, falls überhaupt etwas

journal.window.entities.entity.delete.copy.voice.what.your.journal.keeps.about.its = was dein Journal über ihre Stimme behält

journal.window.entities.examples.first.and.second = z. B. { $first_name } und { $second_name }

journal.window.entities.examples.names = z. B. { $names }

journal.window.entities.faces.chosen.alt = Profilbild von { $entity_name }

journal.window.entities.faces.confirmed.lede = du hast dieses Profilbild für { $entity_name } gewählt.

journal.window.entities.faces.dismissed.lede = du hast kein Profilbild für { $entity_name } gewählt. wenn andere Gesichter auftauchen, die { $entity_name } sein könnten, können sie hier erscheinen.

journal.window.entities.faces.done.chosen = Profilbild für { $entity_name } gespeichert.

journal.window.entities.faces.done.dismissed = kein Profilbild für { $entity_name }.

journal.window.entities.faces.done.rejected = ok, das ist nicht { $entity_name }.

journal.window.entities.faces.face.alt = ein Gesicht aus einer Besprechung am { $day }, das { $entity_name } sein könnte

journal.window.entities.faces.facet.hint = Profilbild für { $entity_name } wählen

journal.window.entities.faces.from.day = aus einer Besprechung am { $day }

journal.window.entities.faces.not.them = nicht { $entity_name }

journal.window.entities.faces.possible.lede = diese Gesichter aus deinen Besprechungen könnten { $entity_name } sein. keins wird verwendet, bis du eins wählst.

journal.window.entities.faces.possible.title = Profilbild für { $entity_name } wählen

journal.window.entities.faces.suggested.title = ist das { $entity_name }?

journal.window.entities.faces.unconfirmed.alt = ein Gesicht aus einer Besprechung, das { $entity_name } sein könnte, nicht bestätigt

journal.window.entities.facet.detach.confirm = { $entity_name } von { $facet_name } lösen? bleibt in deinem Journal, und du kannst es jederzeit wieder anhängen.

journal.window.entities.facet.detach.done = von { $facet_name } gelöst. { $entity_name } ist noch in deinem Journal.

journal.window.entities.facet.empty.heading = noch nichts in { $facet_name }.

journal.window.entities.facet.membership.list = in { $leading_facets } und { $final_facet }

journal.window.entities.facet.membership.single = in { $facet_name }

journal.window.entities.field.label.alternative.names.akas = alternative Namen (auch bekannt als)

journal.window.entities.field.label.description = Beschreibung

journal.window.entities.field.label.name = Name

journal.window.entities.field.label.type = Typ

journal.window.entities.field.placeholder.alias1.alias2.alias3 = alias1, alias2, alias3

journal.window.entities.field.placeholder.e.g.lead.engineer.on.the = z. B. Entwicklungsleitung im Projekt, Hauptkontakt für den Vertrieb…

journal.window.entities.field.placeholder.enter.description = Beschreibung eingeben…

journal.window.entities.field.placeholder.search.entities = Entitäten suchen…

journal.window.entities.field.placeholder.type.a.name.to.find.it = einen Namen tippen, um ihn zu finden

journal.window.entities.folder.all = alle

journal.window.entities.folder.companies = Firmen

journal.window.entities.folder.duplicates = mögliche Duplikate

journal.window.entities.folder.hidden = ausgeblendet

journal.window.entities.folder.other = andere

journal.window.entities.folder.people = Personen

journal.window.entities.folder.projects = Projekte

journal.window.entities.folder.tools = Werkzeuge

journal.window.entities.heading.couldn.t.load.entities = Entitäten konnten nicht geladen werden

journal.window.entities.heading.that.search.couldn.t.be.run = diese Suche konnte nicht ausgeführt werden

journal.window.entities.heading.this.entity.couldn.t.be.loaded = diese Entität konnte nicht geladen werden

journal.window.entities.hide.already.absent.count = { $existing_quantity ->
    [one] 1 war schon ausgeblendet oder weg.
   *[other] { $existing } waren schon ausgeblendet oder weg.
}

journal.window.entities.hide.detail.action.help = { $action_start }ausblenden{ $action_end } { $description_start }verschiebt sie nach ausgeblendet, wo du sie wieder einblenden kannst. ihre Notizen und ihre Erkennungen in ihren Facetten verlassen die Suche, und verbundene Agenten können sie nicht erreichen.{ $description_end }

journal.window.entities.hide.done.count = { $hidden_quantity ->
    [one] 1 Entität ausgeblendet.
   *[other] { $hidden } Entitäten ausgeblendet.
}

journal.window.entities.hide.partial = { $remaining_quantity ->
    [one] { $hidden_quantity ->
        [one] { $hidden }
       *[other] { $hidden }
    } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } ausgeblendet. der Rest war nicht ausgeblendet.
   *[other] { $hidden_quantity ->
        [one] { $hidden }
       *[other] { $hidden }
    } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } ausgeblendet. die übrigen waren nicht ausgeblendet.
}

journal.window.entities.hide.paused.records = { $records_quantity ->
    [one] Ausblenden ist pausiert: ein Datensatz in deinem Journal kann nicht gelesen werden ({ $paths }).
   *[other] Ausblenden ist pausiert: { $records } Datensätze in deinem Journal können nicht gelesen werden ({ $paths }).
}

journal.window.entities.hide.progress = { $done_quantity ->
    [one] wird ausgeblendet… { $done } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    }
   *[other] wird ausgeblendet… { $done } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    }
}

journal.window.entities.hide.record.error = sie konnten nicht ausgeblendet werden: ein Datensatz in deinem Journal kann nicht gelesen werden ({ $record_detail }).

journal.window.entities.hide.selection.count = { $selected_quantity ->
    [one] { $selected } ausblenden
   *[other] { $selected } ausblenden
}

journal.window.entities.hide.selection.progress = { $done_quantity ->
    [one] wird ausgeblendet… { $done } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    }
   *[other] wird ausgeblendet… { $done } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    }
}

journal.window.entities.hide.undo.done = { $restored_quantity ->
    [one] 1 Entität zurückgeholt.
   *[other] { $restored } Entitäten zurückgeholt.
}

journal.window.entities.hide.undo.help = { $hidden_quantity ->
    [one] rückgängig machen holt es zurück, einschließlich der Facetten. es ist auch unter Ausgeblendet, wann immer du es willst.
   *[other] rückgängig machen holt sie zurück, einschließlich der Facetten. sie sind auch unter Ausgeblendet, wann immer du sie willst.
}

journal.window.entities.hide.undo.partial = { $failed_quantity ->
    [one] { $restored_quantity ->
        [one] { $restored }
       *[other] { $restored }
    } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } zurückgeholt. 1 konnte nicht zurückgeholt werden; es ist noch unter Ausgeblendet.
   *[other] { $restored_quantity ->
        [one] { $restored }
       *[other] { $restored }
    } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } zurückgeholt. { $failed } konnten nicht zurückgeholt werden; sie sind noch unter Ausgeblendet.
}

journal.window.entities.hide.undo.progress = { $restored_quantity ->
    [one] wird zurückgeholt… { $restored } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    }
   *[other] wird zurückgeholt… { $restored } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    }
}

journal.window.entities.hide.undo.records.missing.one.entity = { $records_quantity ->
    [one] es ist vielleicht ohne alle Facetten zurückgekommen: ein Datensatz in deinem Journal kann nicht gelesen werden ({ $paths }).
   *[other] es ist vielleicht ohne alle Facetten zurückgekommen: { $records } Datensätze in deinem Journal können nicht gelesen werden ({ $paths }).
}

journal.window.entities.hide.undo.records.missing.several.entities = { $records_quantity ->
    [one] manche sind vielleicht ohne alle Facetten zurückgekommen: ein Datensatz in deinem Journal kann nicht gelesen werden ({ $paths }).
   *[other] manche sind vielleicht ohne alle Facetten zurückgekommen: { $records } Datensätze in deinem Journal können nicht gelesen werden ({ $paths }).
}

journal.window.entities.hide.undo.stopped = { $restored_quantity ->
    [one] rückgängig machen hat aufgehört, nachdem { $restored } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } zurückgeholt wurde. die übrigen sind noch unter Ausgeblendet.
   *[other] rückgängig machen hat aufgehört, nachdem { $restored } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } zurückgeholt wurden. die übrigen sind noch unter Ausgeblendet.
}

journal.window.entities.link.label.back.to.entities = ← zurück zu den Entitäten

journal.window.entities.link.label.edit = bearbeiten

journal.window.entities.link.label.in.all.facets = in allen Facetten →

journal.window.entities.list.heading.facet = Entitäten in { $facet_name }

journal.window.entities.list.last.mentioned.short.day = zuletzt erwähnt { $day }

journal.window.entities.list.page.range = { $first }–{ $last } von { $total }

journal.window.entities.list.results.count = { $matched_quantity ->
    [one] { $matched } von { $total_quantity ->
        [one] { $total } Entität
       *[other] { $total } Entitäten
    }
   *[other] { $matched } von { $total_quantity ->
        [one] { $total } Entität
       *[other] { $total } Entitäten
    }
}

journal.window.entities.list.select.all.count = { $total_quantity ->
    [one] alle { $total } auswählen
   *[other] alle { $total } auswählen
}

journal.window.entities.list.selected.count = { $selected_quantity ->
    [one] { $selected } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } angehakt
   *[other] { $selected } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } angehakt
}

journal.window.entities.list.sort.name = nach Name

journal.window.entities.list.sort.recent = zuletzt

journal.window.entities.list.sort.type = nach Typ

journal.window.entities.merge.confirm.selection = { $selected_quantity ->
    [one] { $selected } zusammenführen
   *[other] { $selected } zusammenführen
}

journal.window.entities.merge.evidence.meeting.photos = Fotos aus Besprechungen legen nahe, dass diese dieselbe Person sein könnten.

journal.window.entities.merge.keep.done = { $total_quantity ->
    [one] 1 Paar getrennt behalten.
   *[other] { $total } Paare getrennt behalten.
}

journal.window.entities.merge.keep.partial = { $failed_quantity ->
    [one] { $processed_quantity ->
        [one] { $processed }
       *[other] { $processed }
    } von { $total_quantity ->
        [one] { $total } Paar
       *[other] { $total } Paaren
    } getrennt behalten. 1 ist noch aufgeführt.
   *[other] { $processed_quantity ->
        [one] { $processed }
       *[other] { $processed }
    } von { $total_quantity ->
        [one] { $total } Paar
       *[other] { $total } Paaren
    } getrennt behalten. { $failed } sind noch aufgeführt.
}

journal.window.entities.merge.keep.selection = { $selected_quantity ->
    [one] { $selected } getrennt behalten
   *[other] { $selected } getrennt behalten
}

journal.window.entities.merge.merge.done = { $total_quantity ->
    [one] 1 Paar zusammengeführt.
   *[other] { $total } Paare zusammengeführt.
}

journal.window.entities.merge.merge.partial = { $failed_quantity ->
    [one] { $processed_quantity ->
        [one] { $processed }
       *[other] { $processed }
    } von { $total_quantity ->
        [one] { $total } Paar
       *[other] { $total } Paaren
    } zusammengeführt. 1 konnte nicht zusammengeführt werden.
   *[other] { $processed_quantity ->
        [one] { $processed }
       *[other] { $processed }
    } von { $total_quantity ->
        [one] { $total } Paar
       *[other] { $total } Paaren
    } zusammengeführt. { $failed } konnten nicht zusammengeführt werden.
}

journal.window.entities.merge.pair.confirm.aria = { $pairs_quantity ->
    [one] 1 Paar zusammenführen?
   *[other] { $pairs } Paare zusammenführen?
}

journal.window.entities.merge.pair.confirm.heading = { $pairs_quantity ->
    [one] 1 Paar zusammenführen?
   *[other] { $pairs } Paare zusammenführen?
}

journal.window.entities.merge.preview.direction = { $source_name } wird in { $target_name } zusammengeführt

journal.window.entities.merge.progress = { $done_quantity ->
    [one] wird zusammengeführt… { $done } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    }
   *[other] wird zusammengeführt… { $done } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    }
}

journal.window.entities.merge.progress.initial = { $total_quantity ->
    [one] wird zusammengeführt… 0 von { $total }
   *[other] wird zusammengeführt… 0 von { $total }
}

journal.window.entities.merge.unmerged.pair.count = { $pairs_quantity ->
    [one] 1 weiteres Paar verlässt die Liste, ohne zusammengeführt zu werden: es enthält eine Entität, die oben zusammengeführt wurde.
   *[other] { $pairs } weitere Paare verlassen die Liste, ohne zusammengeführt zu werden: jedes enthält eine Entität, die oben zusammengeführt wurde.
}

journal.window.entities.message.it.may.already.be.back.in = sie kann schon wieder in deinem Journal sein. lade neu, um zu prüfen.

journal.window.entities.message.it.may.already.be.erased.reload = sie kann schon endgültig gelöscht sein. lade neu, um zu prüfen.

journal.window.entities.message.it.may.already.be.in.the = sie kann schon im Papierkorb sein. lade neu, um zu prüfen.

journal.window.entities.message.it.s.back.in.your.journal = sie ist wieder in deinem Journal.

journal.window.entities.message.it.s.in.the.trash.you = sie ist im Papierkorb. du kannst es von dort noch einmal versuchen.

journal.window.entities.message.it.s.still.in.the.trash = sie ist noch im Papierkorb. du kannst es von dort noch einmal versuchen.

journal.window.entities.message.it.s.still.in.your.journal = sie ist noch in deinem Journal.

journal.window.entities.message.they.may.already.be.back.in = sie sind vielleicht schon wieder in deinem Journal. lade neu, um zu prüfen.

journal.window.entities.message.they.may.already.be.erased.reload = sie sind vielleicht schon endgültig gelöscht. lade neu, um zu prüfen.

journal.window.entities.message.they.may.already.be.in.the = sie sind vielleicht schon im Papierkorb. lade neu, um zu prüfen.

journal.window.entities.message.they.re.back.in.your.journal = sie sind wieder in deinem Journal.

journal.window.entities.message.they.re.still.in.the.trash = sie sind noch im Papierkorb. du kannst es von dort noch einmal versuchen.

journal.window.entities.notes.count = { $notes_quantity ->
    [one] 1 Notiz
   *[other] { $notes } Notizen
}

journal.window.entities.notes.none = keine Notizen

journal.window.entities.observation.coverage = { $observations_quantity ->
    [one] { $observations } Beobachtung über { $days_quantity ->
        [one] { $days } Tag
       *[other] { $days } Tage
    }
   *[other] { $observations } Beobachtungen über { $days_quantity ->
        [one] { $days } Tag
       *[other] { $days } Tage
    }
}

journal.window.entities.observation.disclosure = { $visible_quantity ->
    [one] die { $visible } neuesten von { $total_quantity ->
        [one] { $total } Moment
       *[other] { $total } Momente
    } werden angezeigt
   *[other] die { $visible } neuesten von { $total_quantity ->
        [one] { $total } Moment
       *[other] { $total } Momente
    } werden angezeigt
}

journal.window.entities.observation.grid.count = { $quantity ->
        [one] 1 Moment
       *[other] { $count } Momente
    }

journal.window.entities.observation.latest.day = jüngster Tag: { $day }

journal.window.entities.observation.none.with.entity = noch nichts mit { $entity_name }.

journal.window.entities.observation.range = { $start_day } – { $end_day }

journal.window.entities.observation.show.all = { $observations_quantity ->
    [one] { $observations } Beobachtung anzeigen
   *[other] alle { $observations } Beobachtungen anzeigen
}

journal.window.entities.response.alias.conflict = Alias '{ $alias }' steht in Konflikt mit der Entität '{ $conflicting_name }'.

journal.window.entities.response.alias.conflict.rename = Alias '{ $alias }' steht in Konflikt mit der Entität '{ $conflicting_name }'

journal.window.entities.response.ambiguity.dismiss.status = Mehrdeutigkeit mit Status { $ambiguity_status } kann nicht verworfen werden

journal.window.entities.response.ambiguity.file.corrupt = Mehrdeutigkeitsdatei { $file_path } enthält eine beschädigte Zeile

journal.window.entities.response.candidate.facet.already.exists = Facette '{ $facet }' existiert bereits

journal.window.entities.response.candidate.facet.candidate.accept.failed = der Kandidat konnte nicht angenommen werden

journal.window.entities.response.candidate.facet.candidate.dismiss.failed = der Kandidat konnte nicht verworfen werden

journal.window.entities.response.candidate.facet.candidate.read.failed = der Kandidat konnte nicht gelesen werden

journal.window.entities.response.candidate.facet.facet.creation.failed = die Facette konnte nicht erstellt werden

journal.window.entities.response.candidate.facet.facet.lookup.failed = die Facette konnte nicht nachgeschlagen werden

journal.window.entities.response.candidate.facet.slug.invalid = Ungültiger Facettenname '{ $facet }': muss kleingeschrieben sein, mit einem Buchstaben beginnen und nur Buchstaben, Ziffern, Bindestriche oder Unterstriche enthalten

journal.window.entities.response.candidate.not.found = Kandidat nicht gefunden

journal.window.entities.response.candidate.record.missing = die Kandidaten-Zusammenführung hat keinen passenden aktiven Datensatz

journal.window.entities.response.candidate.status.accept = Kandidat mit Status{ $slot1 }{ $status } kann nicht angenommen werden

journal.window.entities.response.candidate.status.dismiss = Kandidat mit Status{ $slot1 }{ $status } kann nicht verworfen werden

journal.window.entities.response.candidate.status.preview = Vorschau für Kandidat mit Status{ $slot1 }{ $status } ist nicht möglich

journal.window.entities.response.delete.refusal.ambiguities.unreadable = die Mehrdeutigkeitsdatensätze deines Journals konnten nicht gelesen werden

journal.window.entities.response.delete.refusal.bad.request = dieses Löschen konnte nicht gelesen werden

journal.window.entities.response.delete.refusal.busy = dein Journal ist beschäftigt; versuch es in einem Moment noch einmal.

journal.window.entities.response.delete.refusal.failed = dein Journal konnte sie nicht in den Papierkorb verschieben.

journal.window.entities.response.delete.refusal.gone = diese Entität ist nicht in deinem Journal

journal.window.entities.response.delete.refusal.link.unreadable = eine Facettenverknüpfung konnte nicht gelesen werden

journal.window.entities.response.delete.refusal.principal = du kannst dich nicht selbst löschen

journal.window.entities.response.delete.refusal.record.damaged = der Datensatz gelöschter Entitäten deines Journals konnte nicht aktualisiert werden

journal.window.entities.response.detail.access.denied = Zugriff verweigert

journal.window.entities.response.detail.after.id.and.offset.cannot.be.used.together = { $slot1 } und Offset können nicht zusammen verwendet werden

journal.window.entities.response.detail.after.id.must.be.a.non.negative.integer = { $slot1 } muss eine nichtnegative ganze Zahl sein

journal.window.entities.response.detail.aka.is.required = AKA ist erforderlich

journal.window.entities.response.detail.ambiguity.dismiss.failed = die Mehrdeutigkeit konnte nicht verworfen werden

journal.window.entities.response.detail.ambiguity.id.is.required = { $slot1 } ist erforderlich

journal.window.entities.response.detail.ambiguity.not.found = Mehrdeutigkeit nicht gefunden

journal.window.entities.response.detail.ambiguity.read.failed = die Mehrdeutigkeit konnte nicht gelesen werden

journal.window.entities.response.detail.ambiguity.resolve.failed = die Mehrdeutigkeit konnte nicht aufgelöst werden

journal.window.entities.response.detail.batch.block.failed = der Stapel konnte nicht blockiert werden

journal.window.entities.response.detail.batch.unblock.failed = die Blockierung des Stapels konnte nicht aufgehoben werden

journal.window.entities.response.detail.body.is.not.valid.json = der Text ist kein gültiges { $slot1 }

journal.window.entities.response.detail.body.must.be.a.json.object = der Text muss ein { $slot1 }-Objekt sein

journal.window.entities.response.detail.candidate.read.failed = der Kandidat konnte nicht gelesen werden

journal.window.entities.response.detail.content.is.required = Inhalt ist erforderlich

journal.window.entities.response.detail.couldn.t.hide.entity = die Entität konnte nicht ausgeblendet werden

journal.window.entities.response.detail.couldn.t.unhide.entity = die Entität konnte nicht wieder eingeblendet werden

journal.window.entities.response.detail.day.is.required = Tag ist erforderlich

journal.window.entities.response.detail.description.is.required = Beschreibung ist erforderlich

journal.window.entities.response.detail.detected.entity.activity.is.unavailable = Aktivität der erkannten Entität ist nicht verfügbar

journal.window.entities.response.detail.detected.entity.not.found = erkannte Entität nicht gefunden

journal.window.entities.response.detail.detected.entity.save.failed = die erkannte Entität konnte nicht gespeichert werden

journal.window.entities.response.detail.detected.entity.update.failed = die erkannte Entität konnte nicht aktualisiert werden

journal.window.entities.response.detail.detected.read.failed = das Erkannte konnte nicht gelesen werden

journal.window.entities.response.detail.detections.can.t.be.shown.here.which.names.are = Erkennungen können hier nicht angezeigt werden: welche Namen ausgeblendet sind, lässt sich nicht lesen

journal.window.entities.response.detail.entity.alias.update.failed = der Alias der Entität konnte nicht aktualisiert werden

journal.window.entities.response.detail.entity.already.exists = Entität existiert bereits

journal.window.entities.response.detail.entity.already.exists.in.destination.facet.use.merge.to = Entität existiert in der Zielfacette bereits. Nutze { $slot1 } zum Zusammenführen.

journal.window.entities.response.detail.entity.busy = Entität beschäftigt

journal.window.entities.response.detail.entity.create.failed = die Entität konnte nicht erstellt werden

journal.window.entities.response.detail.entity.data.directory.not.found.in.source.facet = Entitätsdatenverzeichnis in der Ursprungsfacette nicht gefunden.

journal.window.entities.response.detail.entity.description.update.failed = die Beschreibung der Entität konnte nicht aktualisiert werden

journal.window.entities.response.detail.entity.detach.failed = die Entität konnte nicht gelöst werden

journal.window.entities.response.detail.entity.hidden = Entität ausgeblendet

journal.window.entities.response.detail.entity.id.is.required = { $slot1 } ist erforderlich

journal.window.entities.response.detail.entity.is.required = Entität ist erforderlich

journal.window.entities.response.detail.entity.move.failed = die Entität konnte nicht verschoben werden

journal.window.entities.response.detail.entity.name.is.invalid = Entitätsname ist ungültig

journal.window.entities.response.detail.entity.not.found = Entität nicht gefunden

journal.window.entities.response.detail.entity.not.found.in.facet = Entität in der Facette nicht gefunden

journal.window.entities.response.detail.entity.not.found.variant2 = Entität nicht gefunden

journal.window.entities.response.detail.entity.read.failed = die Entität konnte nicht gelesen werden

journal.window.entities.response.detail.entity.restore.failed = die Entität konnte nicht wiederhergestellt werden

journal.window.entities.response.detail.entity.search.index.is.busy = der Suchindex der Entitäten ist beschäftigt

journal.window.entities.response.detail.entity.search.index.is.stale = der Suchindex der Entitäten ist veraltet

journal.window.entities.response.detail.entity.search.index.is.unavailable = der Suchindex der Entitäten ist nicht verfügbar

journal.window.entities.response.detail.entity.update.failed = die Entität konnte nicht aktualisiert werden

journal.window.entities.response.detail.entity.with.this.name.already.exists.in.facet = Entität mit diesem Namen existiert in der Facette bereits

journal.window.entities.response.detail.evidence.is.required = Nachweis ist erforderlich

journal.window.entities.response.detail.exclude.name.is.required = { $slot1 } ist erforderlich

journal.window.entities.response.detail.facet.is.required = Facette ist erforderlich

journal.window.entities.response.detail.facet.not.found = Facette nicht gefunden

journal.window.entities.response.detail.facet.read.failed = die Facette konnte nicht gelesen werden

journal.window.entities.response.detail.from.facet.is.required = { $slot1 } ist erforderlich

journal.window.entities.response.detail.grid.read.failed = das Raster konnte nicht gelesen werden

journal.window.entities.response.detail.group.resolve.failed = die Gruppe konnte nicht aufgelöst werden

journal.window.entities.response.detail.hidden.must.be.include.exclude.or.only = { $slot1 } muss { $slot2 }, { $slot3 } oder { $slot4 } sein

journal.window.entities.response.detail.history.read.failed = der Verlauf konnte nicht gelesen werden

journal.window.entities.response.detail.history.version.was.not.found = Verlaufsversion wurde nicht gefunden

journal.window.entities.response.detail.ids.elements.must.be.strings = Elemente von ids müssen Zeichenketten sein

journal.window.entities.response.detail.ids.is.required = ids ist erforderlich

journal.window.entities.response.detail.ids.must.be.an.array = ids muss ein Array sein

journal.window.entities.response.detail.index.plate.task.failed = Index-Plate-Aufgabe ist nicht gelungen

journal.window.entities.response.detail.invalid.detected.entity = ungültige erkannte Entität

journal.window.entities.response.detail.invalid.target.kind = ungültige Zielart

journal.window.entities.response.detail.journal.read.failed = das Journal konnte nicht gelesen werden

journal.window.entities.response.detail.limit.bounded.two.hundred = Limit muss zwischen 0 und 200 liegen

journal.window.entities.response.detail.limit.is.required = Limit ist erforderlich

journal.window.entities.response.detail.limit.must.be.an.integer = das Limit muss eine ganze Zahl sein

journal.window.entities.response.detail.member.ids.is.required = { $slot1 } ist erforderlich

journal.window.entities.response.detail.member.ids.must.contain.only.strings = { $slot1 } darf nur Zeichenketten enthalten

journal.window.entities.response.detail.member.ids.must.not.be.empty = { $slot1 } darf nicht leer sein

journal.window.entities.response.detail.merge.candidate.accept.failed = der Zusammenführungskandidat konnte nicht angenommen werden

journal.window.entities.response.detail.merge.candidate.dismiss.failed = der Zusammenführungskandidat konnte nicht verworfen werden

journal.window.entities.response.detail.merge.candidate.record.failed = der Zusammenführungskandidat konnte nicht gespeichert werden

journal.window.entities.response.detail.merge.commit.failed = die Zusammenführung konnte nicht festgeschrieben werden

journal.window.entities.response.detail.merge.preview.failed = Zusammenführungsvorschau konnte nicht angezeigt werden

journal.window.entities.response.detail.missing.name.key = Fehlt { $slot1 }

journal.window.entities.response.detail.name.is.required = Name ist erforderlich

journal.window.entities.response.detail.no.data.provided = Keine Daten angegeben

journal.window.entities.response.detail.observation.content.cannot.be.empty = Beobachtungsinhalt darf nicht leer sein

journal.window.entities.response.detail.observation.entity.lookup.failed = die Entität der Beobachtung konnte nicht nachgeschlagen werden

journal.window.entities.response.detail.offset.must.be.a.non.negative.integer = Offset muss eine nichtnegative ganze Zahl sein

journal.window.entities.response.detail.offset.must.not.be.negative = Offset darf nicht negativ sein

journal.window.entities.response.detail.old.name.and.new.name.are.required = { $slot1 } und { $slot2 } sind erforderlich

journal.window.entities.response.detail.operation.is.required = Vorgang ist erforderlich

journal.window.entities.response.detail.operation.must.be.a.string = Vorgang muss eine Zeichenkette sein

journal.window.entities.response.detail.recorded.entity.choice.is.absent.from.this.facet = die gespeicherte Entitätsauswahl fehlt in dieser Facette

journal.window.entities.response.detail.recorded.entity.choice.is.hidden = die gespeicherte Entitätsauswahl ist ausgeblendet

journal.window.entities.response.detail.request.body.could.not.be.read = der Anfrageinhalt konnte nicht gelesen werden

journal.window.entities.response.detail.resolution.task.failed = Auflösungsaufgabe ist nicht gelungen

journal.window.entities.response.detail.restore.is.not.available = Wiederherstellen ist nicht verfügbar

journal.window.entities.response.detail.review.item.not.found = Prüfungselement nicht gefunden

journal.window.entities.response.detail.review.read.failed = die Prüfung konnte nicht gelesen werden

journal.window.entities.response.detail.review.restore.failed = die Prüfung konnte nicht wiederhergestellt werden

journal.window.entities.response.detail.review.sweep.failed = Prüfdurchlauf ist nicht gelungen

journal.window.entities.response.detail.revision.is.required = Revision ist erforderlich

journal.window.entities.response.detail.since.must.be.a.yyyymmdd.day = since muss ein { $slot1 }-Tag sein

journal.window.entities.response.detail.sort.must.be.activity.or.name = { $slot1 } muss { $slot2 } oder { $slot3 } sein

journal.window.entities.response.detail.source.and.target.resolve.to.the.same.entity = Quelle und Ziel verweisen auf dieselbe Entität.

journal.window.entities.response.detail.source.is.required = Quelle ist erforderlich

journal.window.entities.response.detail.source.slug.is.required = { $slot1 } ist erforderlich

journal.window.entities.response.detail.status.must.be.open.or.resolved = { $slot1 } muss { $slot2 } oder { $slot3 } sein

journal.window.entities.response.detail.suggestions.are.busy.try.again = Vorschläge sind belegt; versuch es noch einmal

journal.window.entities.response.detail.target.is.required = Ziel ist erforderlich

journal.window.entities.response.detail.target.slug.is.required = { $slot1 } ist erforderlich

journal.window.entities.response.detail.the.name.couldn.t.be.changed.so.nothing.changed = der Name konnte nicht geändert werden, also hat sich nichts geändert

journal.window.entities.response.detail.the.names.hidden.in.this.facet.can.t.be = ausgeblendete Namen in dieser Facette konnten nicht gelesen werden, also änderte sich nichts

journal.window.entities.response.detail.this.entity.is.hidden.so.its.names.can.t = diese Entität ist ausgeblendet, ihre Namen ändern sich erst, wenn sie wieder eingeblendet ist

journal.window.entities.response.detail.this.entity.is.hidden.unhide.it.in.the.entities = Diese Entität ist ausgeblendet. Blende sie zuerst in der Entitäten-App wieder ein.

journal.window.entities.response.detail.to.facet.is.required = { $slot1 } ist erforderlich

journal.window.entities.response.detail.type.and.name.are.required = Typ und Name sind erforderlich

journal.window.entities.response.detail.type.is.required = Typ ist erforderlich

journal.window.entities.response.detail.type.must.be.a.canonical.entity.type.or.other = Typ muss ein kanonischer Entitätstyp oder other sein

journal.window.entities.response.detail.version.id.is.required = { $slot1 } ist erforderlich

journal.window.entities.response.entity.exists = Entität '{ $entity_name }' gibt es bereits

journal.window.entities.response.entity.merged.permanent = '{ $merged_id }' wurde in '{ $survivor_id }' zusammengeführt. Zusammenführungen lassen sich nicht rückgängig machen, also kann dieser Name nicht als neue Entität hinzugefügt werden; verwende stattdessen '{ $survivor_id }'.

journal.window.entities.response.entity.missing = Entität '{ $entity_id }' nicht gefunden

journal.window.entities.response.entity.type.invalid = Ungültiger Entitätstyp { $entity_type }

journal.window.entities.response.entity.type.invalid.query = Ungültiger Entitätstyp: { $entity_type }

journal.window.entities.response.entity.type.invalid.write = Ungültiger Entitätstyp '{ $entity_type }'

journal.window.entities.response.facet.entity.missing = '{ $entity_name }' ist nicht in dieser Facette. füge sie zuerst hier hinzu, dann füge Notizen dazu hinzu.

journal.window.entities.response.ids.count.invalid = ungültige ids-Anzahl: { $id_count }

journal.window.entities.response.merge.refusal.both.principal = diese lassen sich nicht zusammenführen: beide sind als du markiert.

journal.window.entities.response.merge.refusal.person.type = eine Person kann nicht in etwas zusammengeführt werden, das keine Person ist.

journal.window.entities.response.merge.refusal.principal.type = du kannst nicht in etwas zusammengeführt werden, das keine Person ist.

journal.window.entities.response.merge.refusal.same.entity = Quelle und Ziel müssen verschiedene Entitäten sein.

journal.window.entities.response.merge.refusal.source.hidden = etwas Ausgeblendetes lässt sich nicht zusammenführen. blende es zuerst wieder ein.

journal.window.entities.response.merge.refusal.source.missing = Entität der Quelle nicht gefunden:{ $slot1 }{ $id }

journal.window.entities.response.merge.refusal.target.hidden = in etwas Ausgeblendetes lässt sich nicht zusammenführen. blende es zuerst wieder ein.

journal.window.entities.response.merge.refusal.target.missing = Entität des Ziels nicht gefunden:{ $slot1 }{ $id }

journal.window.entities.response.merge.repair = Wende dich an einen Operator, um diese Zusammenführung zu reparieren.

journal.window.entities.response.operation.already.undone = Vorgang { $operation_id } wurde bereits rückgängig gemacht

journal.window.entities.response.operation.id.invalid = ungültige Vorgangs-ID: { $operation_id }

journal.window.entities.response.operation.limit.invalid = ungültiges Limit: { $limit }

journal.window.entities.response.photo.entity.not.found = diese Person konnte nicht gefunden werden.

journal.window.entities.response.photo.face.action.changed = das hat sich geändert, seit die Seite geladen wurde. lade die Seite neu.

journal.window.entities.response.photo.face.not.found = dieses Foto ist nicht mehr verfügbar.

journal.window.entities.response.photo.faces.busy = Gesichtsdaten sind belegt. versuch es in Kürze noch einmal.

journal.window.entities.response.photo.faces.disabled = schalte Profilbildvorschläge ein, bevor du ein Profilbild änderst.

journal.window.entities.response.photo.faces.enabled = schalte Profilbildvorschläge aus, bevor du Gesichtsdaten endgültig löschst.

journal.window.entities.response.photo.faces.read.failed = diese Fotos konnten nicht gelesen werden. versuch es noch einmal.

journal.window.entities.response.photo.request.invalid = diese Fotoanfrage konnte nicht gelesen werden.

journal.window.entities.response.principal.protected = Prinzipalentität ist vor diesem Vorgang geschützt: { $entity_id }

journal.window.entities.response.query.integer.bounded = { $parameter_name } muss zwischen 0 und { $maximum } liegen

journal.window.entities.response.query.integer.required = { $parameter_name } muss eine ganze Zahl sein

journal.window.entities.response.refused = Entitätsanfrage abgelehnt

journal.window.entities.response.summary.limit.bounded = Limit muss zwischen 0 und { $maximum } liegen

journal.window.entities.scope.showing.facet = zeigt { $facet_name } ⌄

journal.window.entities.scope.showing.journal = zeigt dein ganzes Journal ⌄

journal.window.entities.search.finding.names = Entitäten passend zu "{ $query }" werden gesucht…

journal.window.entities.search.finding.type = { $entity_type }-Entitäten werden gesucht…

journal.window.entities.search.finding.type.and.names = { $entity_type }-Entitäten passend zu "{ $query }" werden gesucht…

journal.window.entities.search.none.match = keine Entitäten passen zu "{ $query }".

journal.window.entities.search.placeholder.entity.count = { $total_quantity ->
    [one] { $total } Entität suchen
   *[other] { $total } Entitäten suchen
}

journal.window.entities.section.heading.detach = lösen

journal.window.entities.section.heading.no.entities.yet = noch keine Entitäten

journal.window.entities.state.edit.loading = wird geladen…

journal.window.entities.state.edit.unavailable = nicht verfügbar

journal.window.entities.text.couldn.t.hide.them = sie konnten nicht ausgeblendet werden.

journal.window.entities.text.hiding.stopped.partway.undo.puts.back = Ausblenden stoppte mittendrin. rückgängig machen legt alles zurück, was es ausblendete.

journal.window.entities.text.loading = wird geladen…

journal.window.entities.text.loading.entities = Entitäten werden geladen…

journal.window.entities.text.undo.stopped.before.putting.any.back = rückgängig machen stoppte, bevor etwas zurückgelegt wurde. sie sind noch unter ausgeblendet.

journal.window.entities.tidy.duplicate.count = { $pairs_quantity ->
    [one] { $pairs } mögliches Duplikat
   *[other] { $pairs } mögliche Duplikate
}

journal.window.entities.tidy.group.load.error = diese Gruppe konnte nicht geladen werden.

journal.window.entities.tidy.id.count = { $entities_quantity ->
    [one] { $entities } sieht aus wie eine ID
   *[other] { $entities } sehen aus wie eine ID
}

journal.window.entities.title.couldn.t.erase.those = die konnten nicht endgültig gelöscht werden

journal.window.entities.title.couldn.t.move.that.to.the = das konnte nicht in den Papierkorb verschoben werden

journal.window.entities.title.couldn.t.move.those.to.the = die konnten nicht in den Papierkorb verschoben werden

journal.window.entities.title.couldn.t.recover.those = die konnten nicht wiederhergestellt werden

journal.window.entities.title.delete.this.entity = diese Entität löschen?

journal.window.entities.title.erase.this.entity.now = diese Entität jetzt endgültig löschen?

journal.window.entities.title.moved.to.the.trash = in den Papierkorb verschoben

journal.window.entities.title.your.journal.didn.t.answer = dein Journal hat nicht geantwortet

journal.window.entities.tooltip.add.to.facet = Zu Facette hinzufügen

journal.window.entities.tooltip.cannot.hide.yourself = du kannst dich nicht selbst ausblenden

journal.window.entities.tooltip.entity.is.hidden.and.detached.from = Entität ist ausgeblendet und von dieser Facette gelöst

journal.window.entities.tooltip.entity.type = Typ: { $entity_type }

journal.window.entities.tooltip.facet.name = Facette: { $facet_name }

journal.window.entities.tooltip.has.voiceprint = Hat Stimmabdruck

journal.window.entities.tooltip.last.mentioned.full.day = Zuletzt erwähnt: { $day }

journal.window.entities.tooltip.last.mentioned.short.day = zuletzt erwähnt { $day }

journal.window.entities.tooltip.mention.days = { $mentions_quantity ->
    [one] { $mentions } Erwähnung: in den letzten 30 Tagen erkannte Tage
   *[other] { $mentions } Erwähnungen: in den letzten 30 Tagen erkannte Tage
}

journal.window.entities.tooltip.this.entity.is.hidden = diese Entität ist ausgeblendet

journal.window.entities.tooltip.this.is.you = das bist du

journal.window.entities.tooltip.this.is.your.entity.in.this = Das ist deine Entität in dieser Facette

journal.window.entities.tooltip.unhide.this.entity = diese Entität wieder einblenden

journal.window.entities.tooltip.view.in.facet = Klicke zum Anzeigen in { $facet_name }

journal.window.entities.trust.history.change = { $previous_identity } → { $updated_identity }

journal.window.entities.trust.history.event = { $event_kind } · { $event_time } · { $entity_name }

journal.window.entities.trust.history.suggested.by = vorgeschlagen von { $agent_name }

journal.window.entities.trust.identity.details = { $entity_name } · { $entity_type } · auch bekannt als: { $aliases }

journal.window.entities.trust.merge.confirm = { $source_name } in { $target_name } zusammenführen? eine Zusammenführung lässt sich nicht rückgängig machen.

journal.window.entities.trust.merge.preview.counts = { $voice_samples_quantity ->
    [one] Stimmproben hinzugefügt: { $voice_samples } · Stimmproben nach dem Zusammenführen: { $voice_total_quantity ->
        [one] { $voice_total }
       *[other] { $voice_total }
    } · Notizen unter dem Namen, der zusammengeführt wird: { $notes_quantity ->
        [one] { $notes }
       *[other] { $notes }
    } · Sprecherbeschriftungen geändert: { $speaker_labels_quantity ->
        [one] { $speaker_labels }
       *[other] { $speaker_labels }
    } · Aktivitätsdatensätze geändert: { $activities_quantity ->
        [one] { $activities }
       *[other] { $activities }
    }
   *[other] Stimmproben hinzugefügt: { $voice_samples } · Stimmproben nach dem Zusammenführen: { $voice_total_quantity ->
        [one] { $voice_total }
       *[other] { $voice_total }
    } · Notizen unter dem Namen, der zusammengeführt wird: { $notes_quantity ->
        [one] { $notes }
       *[other] { $notes }
    } · Sprecherbeschriftungen geändert: { $speaker_labels_quantity ->
        [one] { $speaker_labels }
       *[other] { $speaker_labels }
    } · Aktivitätsdatensätze geändert: { $activities_quantity ->
        [one] { $activities }
       *[other] { $activities }
    }
}

journal.window.entities.trust.merge.preview.summary = { $aliases_quantity ->
    [one] Aliasse hinzugefügt: { $aliases } · E-Mails hinzugefügt: { $emails_quantity ->
        [one] { $emails }
       *[other] { $emails }
    }
   *[other] Aliasse hinzugefügt: { $aliases } · E-Mails hinzugefügt: { $emails_quantity ->
        [one] { $emails }
       *[other] { $emails }
    }
}

journal.window.entities.trust.suggestion.details = { $source_name } → { $target_name } · { $facet_name }

journal.window.entities.visible.text.attach.to = anhängen an

journal.window.entities.visible.text.attach.to.facet = an Facette anhängen

journal.window.entities.visible.text.attaching = wird angehängt…

journal.window.entities.visible.text.cancel = abbrechen

journal.window.entities.visible.text.clear.selection = Auswahl aufheben

journal.window.entities.visible.text.comma.separated.list.of.alternative.names = durch Kommas getrennte Liste alternativer Namen für die Audio-Transkription

journal.window.entities.visible.text.connections = Verbindungen

journal.window.entities.visible.text.couldn.t.load.that.page.of = diese Seite von Entitäten konnte nicht geladen werden.

journal.window.entities.visible.text.description = Beschreibung

journal.window.entities.visible.text.detach = lösen

journal.window.entities.visible.text.detached = gelöst

journal.window.entities.visible.text.detected.entities = erkannte Entitäten

journal.window.entities.visible.text.dots.are.the.facets.a.name = Punkte sind die Facetten, in denen ein Name vorkommt.

journal.window.entities.visible.text.each.pair.below.becomes.one.entity = jedes Paar unten wird eine Entität. Zusammenführen lässt sich nicht rückgängig machen.

journal.window.entities.visible.text.edit.entity = Entität bearbeiten

journal.window.entities.visible.text.entities = Entitäten

journal.window.entities.visible.text.facets = Facetten

journal.window.entities.visible.text.failed.to.save.changes = Speichern der Änderungen ist nicht gelungen

journal.window.entities.visible.text.found.by.ai.in.the.last = von KI in den letzten 30 Tagen gefunden. mit einem Stern deinem Journal hinzufügen

journal.window.entities.visible.text.has.voiceprint = Hat Stimmabdruck

journal.window.entities.visible.text.hidden = ausgeblendet

journal.window.entities.visible.text.hide = ausblenden

journal.window.entities.visible.text.hiding = wird ausgeblendet…

journal.window.entities.visible.text.how.does.this.entity.relate.to = (wie bezieht sich diese Entität auf diese Facette?)

journal.window.entities.visible.text.loading = wird geladen…

journal.window.entities.visible.text.loading.entities = Entitäten werden geladen…

journal.window.entities.visible.text.loading.this.entity = diese Entität wird geladen…

journal.window.entities.visible.text.more.about.hidden = mehr über Ausgeblendetes

journal.window.entities.visible.text.name.is.required = Name ist erforderlich

journal.window.entities.visible.text.names.shaped.like.codes.file.names = Namen, die wie Codes, Dateinamen oder Anfragenummern aussehen. keiner hat eine Notiz.

journal.window.entities.visible.text.no.description.click.to.add = keine Beschreibung. klicke zum Hinzufügen…

journal.window.entities.visible.text.no.entities.match.your.search = keine Entitäten passen zu deiner Suche.

journal.window.entities.visible.text.no.entity.matches.that.name = keine Entität passt zu diesem Namen.

journal.window.entities.visible.text.not.attached = nicht angehängt

journal.window.entities.visible.text.not.attached.to.any.facets.yet = noch an keine Facetten angehängt.

journal.window.entities.visible.text.not.now = nicht jetzt

journal.window.entities.visible.text.nothing.here = hier ist nichts.

journal.window.entities.visible.text.only.in.facets.you.muted = nur in Facetten, die du stummgeschaltet hast.

journal.window.entities.visible.text.pairs.that.might.be.the.same = mögliche Paare derselben Person oder Sache. Zusammenführen lässt sich nicht rückgängig machen.

journal.window.entities.visible.text.people.places.and.things.you.re = Personen, Orte und Dinge, die du pflegst

journal.window.entities.visible.text.review = prüfen

journal.window.entities.visible.text.review.the.group = Gruppe prüfen

journal.window.entities.visible.text.save = speichern

journal.window.entities.visible.text.save.changes = Änderungen speichern

journal.window.entities.visible.text.saved = gespeichert!

journal.window.entities.visible.text.saving = wird gespeichert…

journal.window.entities.visible.text.select.a.facet.to.add.entities = Wähle eine Facette aus, um Entitäten deinem Journal hinzuzufügen.

journal.window.entities.visible.text.still.working = arbeitet noch…

journal.window.entities.visible.text.they.move.to.hidden.where.you = sie wechseln zu ausgeblendet, wo du sie wieder einblenden kannst. verbundene Agenten erreichen sie nicht. deine eigene Entität ist nie in dieser Gruppe.

journal.window.entities.visible.text.try.again = erneut versuchen

journal.window.entities.visible.text.undo = rückgängig machen

journal.window.entities.visible.text.unhide = wieder einblenden

journal.window.entities.visible.text.unhiding = wird wieder eingeblendet…

journal.window.entities.visible.text.unticked.ones.stay.as.they.are = nicht Angehakte bleiben, wie sie sind.

journal.window.entities.visible.text.worth.a.look = einen Blick wert

journal.window.entities.visible.text.you = du

journal.window.entities.voice.heading.count = { $samples_quantity ->
    [one] Stimmproben ({ $samples })
   *[other] Stimmproben ({ $samples })
}

journal.window.entities.voice.invalid.key = ungültiger Stimmprobenschlüssel

journal.window.entities.voice.sample.sentence = Satz { $sentence_id }

journal.window.entities.window.explanation.couldn.t.keep.it.separate.it = es konnte nicht getrennt behalten werden. es ist noch gelistet.

journal.window.entities.window.explanation.couldn.t.keep.them.separate.they = sie konnten nicht getrennt behalten werden. sie sind noch gelistet.

journal.window.entities.window.explanation.couldn.t.merge.it.it.s = es konnte nicht zusammengeführt werden. es ist noch gelistet.

journal.window.entities.window.explanation.couldn.t.merge.them.they.re = sie konnten nicht zusammengeführt werden. sie sind noch gelistet.

journal.window.entities.window.explanation.couldn.t.search.for.a.merge = nach einem Zusammenführungsziel konnte nicht gesucht werden.

journal.window.entities.window.explanation.failed.to.load.observations = Beobachtungen konnten nicht geladen werden

journal.window.entities.window.explanation.look.like.an.id = sehen wie eine ID aus

journal.window.entities.window.explanation.this.entity.couldn.t.be.loaded = diese Entität konnte nicht geladen werden, also kann sie nicht bearbeitet werden.

journal.window.entities.window.state.text.couldn.t.attach.entity = die Entität konnte nicht angehängt werden

journal.window.entities.window.state.text.couldn.t.detach.entity = die Entität konnte nicht gelöst werden

journal.window.entities.window.state.text.couldn.t.hide.entity = die Entität konnte nicht ausgeblendet werden

journal.window.entities.window.state.text.couldn.t.re.attach.entity = die Entität konnte nicht erneut angehängt werden

journal.window.entities.window.state.text.couldn.t.save.description = die Beschreibung konnte nicht gespeichert werden

journal.window.entities.window.state.text.couldn.t.unhide.entity = die Entität konnte nicht wieder eingeblendet werden

journal.window.entities.window.state.text.failed.to.attach = Anhängen ist nicht gelungen

journal.window.entities.window.state.text.failed.to.attach.variant2 = Anhängen ist nicht gelungen

journal.window.entities.window.state.text.failed.to.detach = Lösen ist nicht gelungen

journal.window.entities.window.state.text.failed.to.re.attach = Erneutes Anhängen ist nicht gelungen

journal.window.entities.window.state.text.failed.to.save = Speichern ist nicht gelungen

journal.window.entities.window.state.text.finding.entities = Entitäten werden gesucht…

journal.window.entities.window.state.text.loading.this.entity = diese Entität wird geladen…

journal.window.entities.window.state.text.next.page = nächste Seite

journal.window.entities.window.state.text.not.set = nicht festgelegt

journal.window.entities.window.state.text.previous.page = vorherige Seite

journal.window.entities.window.state.text.suggested.match = vorgeschlagene Übereinstimmung

journal.window.entities.window.state.text.try.again = versuch es noch einmal.

journal.window.facets.activities.calendar.scheduled.title = damals in deinem Kalender: { $meeting_title }

journal.window.facets.activities.copy.action.correct = das stimmt nicht

journal.window.facets.activities.copy.action.edit = bearbeiten

journal.window.facets.activities.copy.action.hide = ausblenden

journal.window.facets.activities.copy.action.separate = trennen

journal.window.facets.activities.copy.action.separate.yes = es trennen

journal.window.facets.activities.copy.action.unhide = wieder einblenden

journal.window.facets.activities.copy.back = zurück

journal.window.facets.activities.copy.cancel = abbrechen

journal.window.facets.activities.copy.correct.title = das stimmt nicht

journal.window.facets.activities.copy.correct.what = was tatsächlich passiert ist

journal.window.facets.activities.copy.correct.who = wer es gesagt hat

journal.window.facets.activities.copy.corrected.by.you = von dir korrigiert

journal.window.facets.activities.copy.cue.edited = von dir bearbeitet

journal.window.facets.activities.copy.empty.facet.show.all = alle Facetten anzeigen

journal.window.facets.activities.copy.empty.today.heading = heute noch nichts

journal.window.facets.activities.copy.evidence.open = in Transkripten öffnen ›

journal.window.facets.activities.copy.evidence.open.narrow = öffnen ›

journal.window.facets.activities.copy.heading.accounts = wodurch es festgehalten wurde

journal.window.facets.activities.copy.heading.description = Beschreibung

journal.window.facets.activities.copy.heading.history = Verlauf

journal.window.facets.activities.copy.heading.summary = Zusammenfassung

journal.window.facets.activities.copy.hidden.outline = ausgeblendet

journal.window.facets.activities.copy.history.remove = entfernen

journal.window.facets.activities.copy.history.removed = entfernt

journal.window.facets.activities.copy.history.you.hid = du hast es ausgeblendet

journal.window.facets.activities.copy.history.you.unhid = du hast es wieder eingeblendet

journal.window.facets.activities.copy.home.today.link = alle Aktivitäten von heute →

journal.window.facets.activities.copy.label.description = Beschreibung

journal.window.facets.activities.copy.label.title = Titel

journal.window.facets.activities.copy.lines.loading = die gesprochenen Zeilen werden gesucht…

journal.window.facets.activities.copy.lines.only = nur diese Zeilen

journal.window.facets.activities.copy.lines.others = andere gesprochene Zeilen

journal.window.facets.activities.copy.load.error = Aktivitäten konnten nicht geladen werden

journal.window.facets.activities.copy.loading = Aktivitäten werden geladen…

journal.window.facets.activities.copy.now = jetzt

journal.window.facets.activities.copy.people.loading = die Personen in deinem Journal werden geladen…

journal.window.facets.activities.copy.person.search = Person finden

journal.window.facets.activities.copy.reason.other = ausgeblendet

journal.window.facets.activities.copy.reason.owner = du hast das ausgeblendet

journal.window.facets.activities.copy.save = speichern

journal.window.facets.activities.copy.scope.all = zeigt alle Facetten

journal.window.facets.activities.copy.scope.all.option = alle Facetten

journal.window.facets.activities.copy.sheet.close = schließen

journal.window.facets.activities.copy.show.hidden = Ausgeblendete anzeigen

journal.window.facets.activities.copy.source.unknown = Quelle unbekannt

journal.window.facets.activities.copy.speaker.me = ich

journal.window.facets.activities.copy.speaker.person = eine Person

journal.window.facets.activities.copy.state.removed = entfernt

journal.window.facets.activities.copy.state.removed.by.you = von dir entfernt

journal.window.facets.activities.copy.stop.showing.hidden = Ausgeblendete nicht mehr anzeigen

journal.window.facets.activities.copy.unhide = wieder einblenden

journal.window.facets.activities.copy.untimed.heading = ohne Uhrzeit

journal.window.facets.activities.copy.updating = wird aktualisiert…

journal.window.facets.activities.empty.facet.day = nichts in { $facet_name } an diesem Tag

journal.window.facets.activities.evidence.duration.minutes = { $quantity ->
    [one] { $count } min
   *[other] { $count } min
}

journal.window.facets.activities.evidence.segments.mixed = { $quantity ->
    [one] 1 Segment aus mehr als einer Quelle, { $time_range }
   *[other] { $count } Segmente aus mehr als einer Quelle, { $time_range }
}

journal.window.facets.activities.evidence.segments.named = { $quantity ->
    [one] 1 Segment aus { $stream_name }, { $time_range }
   *[other] { $count } Segmente aus { $stream_name }, { $time_range }
}

journal.window.facets.activities.evidence.segments.unknown = { $quantity ->
    [one] 1 Segment aus einer unbekannten Quelle, { $time_range }
   *[other] { $count } Segmente aus einer unbekannten Quelle, { $time_range }
}

journal.window.facets.activities.evidence.show.all.count = { $quantity ->
    [one] { $count } Segment anzeigen
   *[other] alle { $count } Segmente anzeigen
}

journal.window.facets.activities.hidden.footer.count = { $quantity ->
    [one] { $count } an diesem Tag ausgeblendet
   *[other] { $count } an diesem Tag ausgeblendet
}

journal.window.facets.activities.hidden.showing.count = { $quantity ->
    [one] { $count } ausgeblendet angezeigt
   *[other] { $count } ausgeblendet angezeigt
}

journal.window.facets.activities.history.changed.fields = { $field_group ->
   *[title] du hast den Titel geändert
    [description] du hast die Beschreibung geändert
    [details] du hast deine Schilderung davon geändert
    [title-description] du hast den Titel und die Beschreibung geändert
    [title-details] du hast den Titel und deine Schilderung davon geändert
    [description-details] du hast die Beschreibung und deine Schilderung davon geändert
    [title-description-details] du hast den Titel, die Beschreibung und deine Schilderung davon geändert
}

journal.window.facets.activities.history.changed.unclassified = du hast geändert{ $slot1 }

journal.window.facets.activities.history.corrected.lines = { $quantity ->
    [one] du hast korrigiert, wer 1 Zeile gesagt hat
   *[other] du hast korrigiert, wer { $count } Zeilen gesagt hat
}

journal.window.facets.activities.history.found.stream = gefunden in Segmenten aus { $stream_name }

journal.window.facets.activities.history.remove.description = entfernen: { $recorded_change }

journal.window.facets.activities.history.suggested.agent = vorgeschlagen von { $agent }

journal.window.facets.activities.line.correction.help = deine Antwort gilt für die Zeilen, die du ankreuzst, nicht für ihre Stimmen. um deinem Journal eine Stimme beizubringen oder die Stimmen, die es kennt, aufzuräumen, geh zu { $speakers_start }Sprecher{ $speakers_end }.

journal.window.facets.activities.line.drawer.count = { $quantity ->
        [one] 1 Zeile
       *[other] { $count } Zeilen
    }

journal.window.facets.activities.line.play.time = Zeile um { $time } abspielen

journal.window.facets.activities.line.question.selected = { $quantity ->
    [one] wer hat die Zeile gesagt, die du angehakt hast?
   *[other] wer hat die { $count } Zeilen gesagt, die du angehakt hast?
}

journal.window.facets.activities.months.short.apr = Apr

journal.window.facets.activities.months.short.aug = Aug

journal.window.facets.activities.months.short.dec = Dez

journal.window.facets.activities.months.short.feb = Feb

journal.window.facets.activities.months.short.jan = Jan

journal.window.facets.activities.months.short.jul = Jul

journal.window.facets.activities.months.short.jun = Jun

journal.window.facets.activities.months.short.mar = Mär

journal.window.facets.activities.months.short.may = Mai

journal.window.facets.activities.months.short.nov = Nov

journal.window.facets.activities.months.short.oct = Okt

journal.window.facets.activities.months.short.sep = Sep

journal.window.facets.activities.people.recorded.more = { $quantity ->
    [one] mit { $first_person }, { $second_person } und 1 weiterer Person
   *[other] mit { $first_person }, { $second_person } und { $others } weiteren Personen
}

journal.window.facets.activities.people.recorded.one = mit { $person }

journal.window.facets.activities.people.recorded.two = mit { $first_person } und { $second_person }

journal.window.facets.activities.place.recorded.then = { $device_name } war damals { $place }

journal.window.facets.activities.response.detail.disallowed.fields = Patch enthält nicht erlaubte Felder: { $field_list }

journal.window.facets.activities.response.detail.previous.answer.unreadable = die Antwort, die diese Bearbeitung ersetzt hat, kann nicht gelesen werden: { $read_error }

journal.window.facets.activities.response.detail.segment.outside.activity = { $segment_path } ist kein Segment dieser Aktivität

journal.window.facets.activities.response.error.detail.activities.couldn.t.be.updated.right = Aktivitäten konnten gerade nicht aktualisiert werden, weil sie beschäftigt waren. versuch es gleich noch einmal.

journal.window.facets.activities.response.error.detail.day.must.not.be.after.today = Tag darf nicht nach heute liegen

journal.window.facets.activities.response.error.detail.day.not.found = Tag nicht gefunden

journal.window.facets.activities.response.error.detail.each.line.names.its.stream.segment = jede Zeile nennt ihren Datenstrom, ihr Segment, ihre Quelle und sentence_id

journal.window.facets.activities.response.error.detail.facet.must.not.be.empty = Facette darf nicht leer sein

journal.window.facets.activities.response.error.detail.invalid.month.format.expected.yyyymm = Ungültiges Monatsformat, erwartet { $slot1 }

journal.window.facets.activities.response.error.detail.lines.must.be.a.list.of = Zeilen müssen eine Liste von Zeilen sein

journal.window.facets.activities.response.error.detail.only.a.record.in.a.group = nur ein Datensatz in einer Gruppe von zwei oder mehr kann getrennt werden

journal.window.facets.activities.response.error.detail.patch.must.be.a.non.empty = Patch muss ein nicht leeres Objekt sein

journal.window.facets.activities.response.error.detail.patch.must.not.be.empty = Patch darf nicht leer sein

journal.window.facets.activities.response.error.detail.playback.must.be.true.or.false = Wiedergabe muss wahr oder falsch sein

journal.window.facets.activities.response.error.detail.position.timestamp.and.actor.name.the = Position, Zeitstempel und Akteur bezeichnen die zu entfernende Bearbeitung

journal.window.facets.activities.response.error.detail.removing.an.edit.needs.the.owner = eine Bearbeitung entfernen braucht dich

journal.window.facets.activities.response.error.detail.saying.who.said.a.line.needs = sagen, wer eine Zeile gesagt hat, braucht dich

journal.window.facets.activities.response.error.detail.separating.a.group.needs.the.owner = eine Gruppe trennen braucht dich

journal.window.facets.activities.response.error.detail.source.must.be.user = Quelle muss { $slot1 } sein

journal.window.facets.activities.response.error.detail.speaker.is.kind.owner.kind.person = Sprecher ist { $slot1 }, { $slot2 } oder { $slot3 }

journal.window.facets.activities.response.error.detail.title.must.not.be.empty = Titel darf nicht leer sein

journal.window.facets.activities.response.error.message.activities.couldn.t.be.updated.right = Aktivitäten konnten gerade nicht aktualisiert werden, weil sie beschäftigt waren. versuch es gleich noch einmal.

journal.window.facets.activities.response.error.message.only.you.can.remove.a.correction = nur du kannst eine Korrektur entfernen.

journal.window.facets.activities.response.error.message.only.you.can.say.who.said = nur du kannst sagen, wer diese Zeilen gesagt hat.

journal.window.facets.activities.response.error.message.only.you.can.separate.a.group = nur du kannst eine Gruppe trennen.

journal.window.facets.activities.response.error.message.that.activity.couldn.t.be.created = diese Aktivität konnte nicht erstellt werden, weil sie schon existiert.

journal.window.facets.activities.response.error.message.that.activity.isn.t.in.the = diese Aktivität ist nicht in der Facette.

journal.window.facets.activities.response.error.message.that.activity.setting.couldn.t.be = diese Aktivitätseinstellung konnte nicht verwendet werden.

journal.window.facets.activities.response.error.message.that.day.couldn.t.be.used = dieser Tag konnte nicht verwendet werden.

journal.window.facets.activities.response.error.message.that.edit.couldn.t.be.removed = diese Bearbeitung konnte nicht entfernt werden.

journal.window.facets.activities.response.error.message.that.month.couldn.t.be.used = dieser Monat konnte nicht verwendet werden.

journal.window.facets.activities.response.error.message.that.request.didn.t.finish = diese Anfrage wurde nicht fertig.

journal.window.facets.activities.response.error.message.this.isn.t.grouped.with.anything = das ist mit nichts gruppiert.

journal.window.facets.activities.response.error.message.this.record.is.being.moved.you = dieser Datensatz wird verschoben. du kannst ihn bearbeiten, sobald das abgeschlossen ist.

journal.window.facets.activities.response.error.message.who.said.those.lines.couldn.t = wer diese Zeilen gesagt hat, konnte nicht gespeichert werden.

journal.window.facets.activities.scope.selected.facet = zeigt { $facet_name }

journal.window.facets.activities.timeline.item.hidden = { $time_range }, { $facet_name }, ausgeblendet: { $activity_title }

journal.window.facets.activities.timeline.item.hidden.source = { $time_range }, { $facet_name }, { $stream_name }, ausgeblendet: { $activity_title }

journal.window.facets.activities.timeline.item.visible = { $time_range }, { $facet_name }: { $activity_title }

journal.window.facets.activities.timeline.item.visible.source = { $time_range }, { $facet_name }, { $stream_name }: { $activity_title }

journal.window.facets.activities.window.explanation.a.line.shows.the.wrong.person = eine Zeile zeigt die falsche Person

journal.window.facets.activities.window.explanation.activities.appear.here.a.little.while = Aktivitäten erscheinen hier eine Weile nach ihrem Ende und können noch wachsen, während mehr dazukommt.

journal.window.facets.activities.window.explanation.added.with.the.solstone.command = mit dem solstone-Befehl hinzugefügt

journal.window.facets.activities.window.explanation.as.the.day.goes.on.activities = im Laufe des Tages erscheinen hier Aktivitäten aus dem, was du teilst.

journal.window.facets.activities.window.explanation.can.t.be.read.right.now = kann gerade nicht gelesen werden

journal.window.facets.activities.window.explanation.changed.with.the.solstone.command = mit dem solstone-Befehl geändert

journal.window.facets.activities.window.explanation.couldn.t.load.the.people.in = die Personen in deinem Journal konnten nicht geladen werden.

journal.window.facets.activities.window.explanation.couldn.t.remove.that.try.again = das konnte nicht entfernt werden. versuch es gleich noch einmal.

journal.window.facets.activities.window.explanation.couldn.t.save.that.try.again = das konnte nicht gespeichert werden. versuch es gleich noch einmal.

journal.window.facets.activities.window.explanation.couldn.t.save.your.changes.are = das Speichern ist nicht gelungen. deine Änderungen sind noch hier, du kannst erneut versuchen.

journal.window.facets.activities.window.explanation.found.in.segments.from.an.unknown = gefunden in Segmenten aus einer unbekannten Quelle

journal.window.facets.activities.window.explanation.found.in.segments.from.more.than = gefunden in Segmenten aus mehr als einer Quelle

journal.window.facets.activities.window.explanation.grew.as.more.of.it.came = wuchs, als mehr davon dazukam

journal.window.facets.activities.window.explanation.hidden.it.s.still.in.your = ausgeblendet. es ist noch in deinem Journal.

journal.window.facets.activities.window.explanation.hidden.with.the.solstone.command = mit dem solstone-Befehl ausgeblendet

journal.window.facets.activities.window.explanation.it.won.t.join.this.group = es tritt dieser Gruppe nicht wieder bei. nichts wird gelöscht.

journal.window.facets.activities.window.explanation.lines.your.journal.credits.to.you = Zeilen, die dein Journal dir zuschreibt

journal.window.facets.activities.window.explanation.more.than.one.source = mehr als eine Quelle

journal.window.facets.activities.window.explanation.no.activities.on.this.day = keine Aktivitäten an diesem Tag

journal.window.facets.activities.window.explanation.no.longer.in.your.journal = nicht mehr in deinem Journal

journal.window.facets.activities.window.explanation.no.one.in.your.journal.matches = niemand in deinem Journal stimmt damit überein.

journal.window.facets.activities.window.explanation.on.your.calendar.then = damals in deinem Kalender

journal.window.facets.activities.window.explanation.other.facets.have.activities.on.this = andere Facetten haben an diesem Tag Aktivitäten.

journal.window.facets.activities.window.explanation.part.of.it.removed = ein Teil davon entfernt

journal.window.facets.activities.window.explanation.pick.an.activity.to.see.what = wähle eine Aktivität, um zu sehen, worauf sie beruht.

journal.window.facets.activities.window.explanation.say.it.the.way.you.d = sag es so, wie du es einem Freund erzählen würdest: wer was getan hat, was entschieden wurde, was noch offen ist.

journal.window.facets.activities.window.explanation.seen.as.cancelled.or.declined = als abgesagt oder abgelehnt gesehen

journal.window.facets.activities.window.explanation.some.of.those.lines.aren.t = einige dieser Zeilen gehören nicht zu dieser Aktivität.

journal.window.facets.activities.window.explanation.someone.i.don.t.know = jemand, den ich nicht kenne

journal.window.facets.activities.window.explanation.tell.your.journal.in.your.own = erzähl deinem Journal in deinen eigenen Worten

journal.window.facets.activities.window.explanation.that.changed.since.it.was.shown = das hat sich geändert, seit es angezeigt wurde. so steht es jetzt.

journal.window.facets.activities.window.explanation.that.person.can.t.be.chosen = diese Person kann nicht gewählt werden. wähle jemanden anderen oder jemanden, den du nicht kennst.

journal.window.facets.activities.window.explanation.the.journal.adds.activities.as.it = das Journal fügt Aktivitäten hinzu, während es durcharbeitet, was du geteilt hast.

journal.window.facets.activities.window.explanation.these.lines.can.t.take.an = diese Zeilen können keine Antwort annehmen, wer sie gesagt hat. du kannst deinem Journal stattdessen erzählen, was passiert ist.

journal.window.facets.activities.window.explanation.these.lines.have.changed.since.they = diese Zeilen haben sich geändert, seit sie angezeigt wurden. geh zurück und öffne sie noch einmal.

journal.window.facets.activities.window.explanation.this.was.a.recording.or.video = das war eine laufende Aufnahme oder ein laufendes Video

journal.window.facets.activities.window.explanation.tick.the.lines.you.want.to = kreuze zuerst die Zeilen an, die du korrigieren willst.

journal.window.facets.activities.window.explanation.unhidden.with.the.solstone.command = mit dem solstone-Befehl wieder eingeblendet

journal.window.facets.activities.window.explanation.unhide.the.day.on.the.body = blende den Tag auf der Seite Körper wieder ein

journal.window.facets.activities.window.explanation.what.it.s.based.on = worauf es beruht

journal.window.facets.activities.window.explanation.who.said.the.lines.you.ticked = wer hat die Zeilen gesagt, die du angekreuzt hast?

journal.window.facets.activities.window.explanation.yes.it.was.playing = ja, es lief

journal.window.facets.activities.window.explanation.you.hid.this.day.s.body = du hast die Körperdaten dieses Tages ausgeblendet

journal.window.facets.activities.window.explanation.you.said.this.was.a.recording = du hast gesagt, das war eine laufende Aufnahme oder ein laufendes Video

journal.window.facets.activities.window.explanation.you.told.your.journal.what.happened = du hast deinem Journal erzählt, was passiert ist

journal.window.facets.activities.window.explanation.your.account.of.it = deine Schilderung davon

journal.window.facets.activities.window.explanation.your.earlier.answers.about.who.spoke = deine früheren Antworten, wer hier sprach, können nicht gelesen werden, also wurde diese nicht gespeichert.

journal.window.facets.activities.window.explanation.your.journal.doesn.t.know.which = dein Journal weiß noch nicht, welche Person du bist, also kann es Zeilen nicht als deine markieren.

journal.window.facets.activities.window.explanation.your.journal.will.think.this.through = dein Journal wird das noch einmal durchdenken, als etwas, das du angesehen oder angehört hast, ohne dass jemand sonst beteiligt ist.

journal.window.facets.awareness.response.detail.multiple.import.answers = genau eines von { $slot1 } angeben; erhalten { $answer_list }

journal.window.facets.awareness.response.detail.unknown.section = kein Awareness-Abschnitt namens '{ $section_name }'

journal.window.facets.awareness.response.detail.unreadable.settings = deine Einstellungsdatei unter { $settings_path } konnte nicht gelesen werden. deine Einstellungen wurden nicht geändert. repariere die Datei oder stelle { $slot3 } aus einer Sicherung wieder her, und versuch es dann noch einmal.

journal.window.facets.awareness.response.error.detail.kind.is.required = Art ist erforderlich

journal.window.facets.awareness.response.error.detail.no.request.body = kein Anfrageinhalt

journal.window.facets.awareness.response.error.detail.request.body.must.be.a.json = Anfrageinhalt muss ein { $slot1 }-Objekt sein

journal.window.facets.awareness.response.error.message.a.required.field.is.missing = ein erforderliches Feld fehlt.

journal.window.facets.awareness.response.error.message.one.of.those.values.couldn.t = einer dieser Werte konnte nicht verwendet werden.

journal.window.facets.awareness.response.error.message.that.json.request.couldn.t.be = diese { $slot1 }-Anfrage konnte nicht gelesen werden.

journal.window.facets.awareness.response.error.message.that.part.of.your.journal.couldn = dieser Teil deines Journals konnte nicht gefunden werden.

journal.window.facets.awareness.response.error.message.that.request.didn.t.finish = diese Anfrage wurde nicht fertig.

journal.window.facets.awareness.response.error.message.that.request.had.no.data.in = diese Anfrage enthielt keine Daten.

journal.window.facets.awareness.response.error.message.the.awareness.operation.is.busy = Der Awareness-Vorgang ist beschäftigt.

journal.window.facets.awareness.response.error.message.your.settings.couldn.t.be.read = deine Einstellungen konnten nicht gelesen werden.

journal.window.facets.awareness.response.imports.busy.detail = Importe sind beschäftigt; versuch es noch einmal

journal.window.facets.awareness.response.one.import.answer = genau eines von { $slot1 } angeben

journal.window.facets.curation.ambiguity.choose.name = { $candidate_name } wählen

journal.window.facets.curation.ambiguity.member.occurrences = { $quantity ->
    [one] { $count } Vorkommen
   *[other] { $count } Vorkommen
}

journal.window.facets.curation.ambiguity.occurrences = { $quantity ->
    [one] { $count } Vorkommen
   *[other] { $count } Vorkommen
}

journal.window.facets.curation.ambiguity.question = welcher Eintrag stimmt mit "{ $ambiguous_name }" überein?

journal.window.facets.curation.button.label.try.again = erneut versuchen

journal.window.facets.curation.disclosure.label.individual.questions.and.sources = einzelne Fragen und Quellen

journal.window.facets.curation.disclosure.label.source.evidence = Quellennachweis

journal.window.facets.curation.disclosure.label.source.identifiers = Quellenkennungen

journal.window.facets.curation.disclosure.label.technical.details = technische Details

journal.window.facets.curation.evidence.drawer.title = Nachweis

journal.window.facets.curation.evidence.showing.partial = { $shown_quantity ->
    [one] { $shown } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } werden angezeigt
   *[other] { $shown } von { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } werden angezeigt
}

journal.window.facets.curation.evidence.total.pieces = { $quantity ->
    [one] 1 Stück
   *[other] { $count } Stücke
}

journal.window.facets.curation.facet.additional.names = auch als { $alias_list } gesehen.

journal.window.facets.curation.facet.create.question = Journalaktivität passt nicht gut zu deinen Facetten. die Facette "{ $facet_name }" erstellen?

journal.window.facets.curation.list.items = { $preceding_items } und { $last_item }

journal.window.facets.curation.placeholder.this.is.a.speaker.label.not = das ist eine Sprecherbeschriftung, kein Name

journal.window.facets.curation.preview.activities.count = { $quantity ->
    [one] Aktivitätsdatensätze geändert: { $count }
   *[other] Aktivitätsdatensätze geändert: { $count }
}

journal.window.facets.curation.preview.aliases = Aliasse hinzugefügt: { $aliases }

journal.window.facets.curation.preview.emails.count = { $quantity ->
    [one] E-Mails hinzugefügt: { $count }
   *[other] E-Mails hinzugefügt: { $count }
}

journal.window.facets.curation.preview.facet.links = { $moved_quantity ->
    [one] Facettenverknüpfungen: { $moved } verschoben, { $merged_quantity ->
        [one] { $merged } zusammengeführt
       *[other] { $merged } zusammengeführt
    }
   *[other] Facettenverknüpfungen: { $moved } verschoben, { $merged_quantity ->
        [one] { $merged } zusammengeführt
       *[other] { $merged } zusammengeführt
    }
}

journal.window.facets.curation.preview.notes.count = { $quantity ->
    [one] Notizen unter dem Namen, der zusammengeführt wird: { $count }
   *[other] Notizen unter dem Namen, der zusammengeführt wird: { $count }
}

journal.window.facets.curation.preview.observations.count = { $quantity ->
    [one] Notizen verschoben: { $count }
   *[other] Notizen verschoben: { $count }
}

journal.window.facets.curation.preview.speaker.labels.count = { $quantity ->
    [one] Sprecherbeschriftungen geändert: { $count }
   *[other] Sprecherbeschriftungen geändert: { $count }
}

journal.window.facets.curation.preview.speaker.segments = { $labels_quantity ->
    [one] Sprecherbeschriftungen aktualisiert: { $labels } Beschriftung, { $corrections_quantity ->
        [one] { $corrections } Korrektur
       *[other] { $corrections } Korrekturen
    }
   *[other] Sprecherbeschriftungen aktualisiert: { $labels } Beschriftungen, { $corrections_quantity ->
        [one] { $corrections } Korrektur
       *[other] { $corrections } Korrekturen
    }
}

journal.window.facets.curation.preview.voice.samples = { $added_quantity ->
    [one] Stimmproben verschoben: { $added } hinzugefügt, { $total_quantity ->
        [one] { $total } insgesamt
       *[other] { $total } insgesamt
    }
   *[other] Stimmproben verschoben: { $added } hinzugefügt, { $total_quantity ->
        [one] { $total } insgesamt
       *[other] { $total } insgesamt
    }
}

journal.window.facets.curation.response.batch.candidate.identity.missing = dem Kandidaten fehlen Facette, source_slug oder target_slug

journal.window.facets.curation.response.candidate.absent = Kandidat nicht gefunden

journal.window.facets.curation.response.candidate.accept.status = Kandidat mit Status{ $slot1 }{ $status } kann nicht angenommen werden

journal.window.facets.curation.response.candidate.dismiss.status = Kandidat mit Status{ $slot1 }{ $status } kann nicht verworfen werden

journal.window.facets.curation.response.candidate.kept.name.missing = "{ $name }" ist schon in "{ $kept }" zusammengeführt, ohne den Namen zu behalten. füge "{ $name }" auf der Seite Entitäten zu den alternativen Namen von "{ $kept }" hinzu und nimm es dann erneut an.

journal.window.facets.curation.response.candidate.preview.status = Vorschau für Kandidat mit Status{ $slot1 }{ $status } ist nicht möglich

journal.window.facets.curation.response.copy.action.failed = das ist nicht durchgegangen. versuch es noch einmal.

journal.window.facets.curation.response.copy.ambiguity.body = welcher Eintrag stimmt mit "{ $slot1 }" überein?

journal.window.facets.curation.response.copy.ambiguity.choose.action = { $slot1 } wählen

journal.window.facets.curation.response.copy.ambiguity.dismiss.action = nichts davon

journal.window.facets.curation.response.copy.ambiguity.origin.label = bemerkt in

journal.window.facets.curation.response.copy.empty.state = nichts zum Prüfen. noch keine neue Struktur vorzuschlagen.

journal.window.facets.curation.response.copy.entity.batch.confirm.dismiss.action = alle getrennt behalten

journal.window.facets.curation.response.copy.entity.batch.confirm.merge.action = alle zusammenführen

journal.window.facets.curation.response.copy.entity.batch.dismiss.action = Ausgewählte getrennt behalten

journal.window.facets.curation.response.copy.entity.batch.dismiss.lead = diese Paare werden getrennt behalten:

journal.window.facets.curation.response.copy.entity.batch.dismiss.summary = { $slot1 } von { $slot2 } getrennt behalten.

journal.window.facets.curation.response.copy.entity.batch.failed.note = { $slot1 } brauchen unten noch Aufmerksamkeit.

journal.window.facets.curation.response.copy.entity.batch.merge.action = Ausgewählte zusammenführen

journal.window.facets.curation.response.copy.entity.batch.merge.lead = diese Paare werden zusammengeführt. das lässt sich nicht rückgängig machen:

journal.window.facets.curation.response.copy.entity.batch.merge.summary = { $slot1 } von { $slot2 } zusammengeführt.

journal.window.facets.curation.response.copy.entity.body = "{ $slot1 }" und "{ $slot2 }" sehen aus wie dieselbe Entität. sie zusammenführen?

journal.window.facets.curation.response.copy.entity.cancel.action = abbrechen

journal.window.facets.curation.response.copy.entity.confirm.action = Zusammenführen bestätigen

journal.window.facets.curation.response.copy.entity.dismiss.action = getrennt behalten

journal.window.facets.curation.response.copy.entity.done.for.now.action = fürs Erste fertig

journal.window.facets.curation.response.copy.entity.facets.label = (in { $slot1 })

journal.window.facets.curation.response.copy.entity.group.lede = diese { $slot1 } Paare sehen sich ähnlich. sie zusammenzuführen hält deine Namen in Ordnung.

journal.window.facets.curation.response.copy.entity.merge.action = zusammenführen

journal.window.facets.curation.response.copy.entity.preview.empty = für dieses Zusammenführen sind keine Änderungen im Journal nötig.

journal.window.facets.curation.response.copy.entity.preview.errors = einige Segmentaktualisierungen brauchen vielleicht Aufmerksamkeit.

journal.window.facets.curation.response.copy.entity.preview.lead = vor dem Zusammenführen steht hier, was sich ändert. das lässt sich nicht rückgängig machen.

journal.window.facets.curation.response.copy.entity.progress.label = { $slot1 } geprüft · { $slot2 } übrig

journal.window.facets.curation.response.copy.entity.select.all.action = alle auswählen

journal.window.facets.curation.response.copy.entity.show.rest.action = Rest anzeigen ({ $slot1 })

journal.window.facets.curation.response.copy.facet.body = Journalaktivität passt nicht gut zu deinen Facetten. die Facette "{ $slot1 }" erstellen?

journal.window.facets.curation.response.copy.facet.create.action = Facette erstellen

journal.window.facets.curation.response.copy.facet.dismiss.action = nicht jetzt

journal.window.facets.curation.response.copy.heading = Vorschläge

journal.window.facets.curation.response.copy.preview.activities = Aktivitätsdatensätze geändert

journal.window.facets.curation.response.copy.preview.akas.label = Aliasse hinzugefügt

journal.window.facets.curation.response.copy.preview.emails.label = E-Mails hinzugefügt

journal.window.facets.curation.response.copy.preview.facets.label = Facettenverknüpfungen

journal.window.facets.curation.response.copy.preview.labels = Sprecherbeschriftungen geändert

journal.window.facets.curation.response.copy.preview.notes.source = Notizen unter dem Namen, der zusammengeführt wird

journal.window.facets.curation.response.copy.preview.observations.label = Notizen verschoben

journal.window.facets.curation.response.copy.preview.partial = das Zusammenführen führt auch Facettenverknüpfungen und Fotos zusammen.

journal.window.facets.curation.response.copy.preview.segments.label = Sprecherbeschriftungen aktualisiert

journal.window.facets.curation.response.copy.preview.voiceprints.label = Stimmproben verschoben

journal.window.facets.curation.response.copy.repair.required = { $slot1 } { $slot2 }

journal.window.facets.curation.response.copy.review.restore.action = zum Prüfen wiederherstellen

journal.window.facets.curation.response.copy.speaker.body = "{ $slot1 }" und "{ $slot2 }" könnten dieselbe sprechende Person sein. sie zusammenführen?

journal.window.facets.curation.response.copy.speaker.candidate.pair.body = zwei Stimmen in deinem Journal klingen ähnlich. sie zusammenführen?

journal.window.facets.curation.response.copy.speaker.candidate.pair.dismiss.action = getrennt behalten

journal.window.facets.curation.response.copy.speaker.candidate.pair.intervals.label = Intervalle

journal.window.facets.curation.response.copy.speaker.candidate.pair.merge.action = Kandidaten zusammenführen

journal.window.facets.curation.response.copy.speaker.candidate.pair.similarity.label = Kosinus

journal.window.facets.curation.response.copy.speaker.candidate.pair.source.label = Kandidat A

journal.window.facets.curation.response.copy.speaker.candidate.pair.target.label = Kandidat B

journal.window.facets.curation.response.copy.speaker.dismiss.action = getrennt behalten

journal.window.facets.curation.response.copy.speaker.merge.action = Zusammenführen prüfen

journal.window.facets.curation.response.detail.missing.field = Es fehlt { $field_name }

journal.window.facets.curation.response.detail.unknown.group = unbekannte Kurationsgruppe: { $group }

journal.window.facets.curation.response.detail.unreadable.settings = deine Einstellungsdatei unter { $settings_path } konnte nicht gelesen werden. deine Einstellungen wurden nicht geändert. repariere die Datei oder stelle { $slot3 } aus einer Sicherung wieder her, und versuch es dann noch einmal.

journal.window.facets.curation.response.entity.busy.detail = Kandidaten der Entitätszusammenführung sind beschäftigt; erneut versuchen

journal.window.facets.curation.response.error.detail.key.does.not.match.anchor.a = Schlüssel stimmt nicht mit { $slot1 } überein

journal.window.facets.curation.response.error.detail.key.does.not.match.source.id = Schlüssel stimmt nicht mit { $slot1 } überein

journal.window.facets.curation.response.error.message.a.required.field.is.missing = ein erforderliches Feld fehlt.

journal.window.facets.curation.response.error.message.one.of.those.values.couldn.t = einer dieser Werte konnte nicht verwendet werden.

journal.window.facets.curation.response.error.message.that.entity.operation.couldn.t.be = diese Entitätsoperation konnte nicht abgeschlossen werden.

journal.window.facets.curation.response.error.message.that.list.isn.t.one.curation = diese Liste ist keine, die die Kuration durchblättert.

journal.window.facets.curation.response.error.message.the.entity.operation.is.busy = Die Entitätsoperation ist beschäftigt.

journal.window.facets.curation.response.error.message.your.settings.couldn.t.be.read = deine Einstellungen konnten nicht gelesen werden.

journal.window.facets.curation.response.pair.already.confirmed = zwei bestätigte Kandidaten können nicht zusammengeführt werden

journal.window.facets.curation.response.pair.already.merged = Kandidatenpaar ist schon zusammengeführt

journal.window.facets.curation.response.pair.anchor.absent = Kandidatenanker nicht gefunden

journal.window.facets.curation.response.pair.below.threshold = Kandidatenpaar liegt unter der Prüfschwelle

journal.window.facets.curation.response.pair.rejected = abgelehnter Kandidat kann nicht zusammengeführt werden

journal.window.facets.curation.response.speaker.busy.detail = Vorschläge für Sprecher sind belegt; versuch es noch einmal.

journal.window.facets.curation.review.total = { $quantity ->
    [one] { $count } Vorschlag zum Prüfen. wähle eine Gruppe, um anzufangen.
   *[other] { $count } Vorschläge zum Prüfen. wähle eine Gruppe, um anzufangen.
}

journal.window.facets.curation.speaker.merge.question = "{ $source_name }" und "{ $target_name }" könnten derselbe Sprecher sein. sie zusammenführen?

journal.window.facets.curation.status.text.journal = Journal

journal.window.facets.curation.status.text.please.select.at.least.one.question = bitte wähle mindestens eine Frage zum Klären aus.

journal.window.facets.curation.status.text.restore.to.review = zum Prüfen wiederherstellen

journal.window.facets.curation.suggestion.display.text.another.change.is.in.progress.try = eine andere Änderung läuft. versuch es gleich noch einmal.

journal.window.facets.curation.suggestion.display.text.automatic.review = automatische Prüfung

journal.window.facets.curation.suggestion.display.text.couldn.t.apply.that.choice.refresh = diese Auswahl konnte nicht angewendet werden. aktualisiere die Vorschläge und versuch es noch einmal.

journal.window.facets.curation.suggestion.display.text.couldn.t.save.that.change.try = diese Änderung konnte nicht gespeichert werden. versuch es noch einmal.

journal.window.facets.curation.suggestion.display.text.facet.suggestions = Facettenvorschläge

journal.window.facets.curation.suggestion.display.text.names.to.clarify = Namen zum Klären

journal.window.facets.curation.suggestion.display.text.possible.duplicate.voices = mögliche doppelte Stimmen

journal.window.facets.curation.suggestion.display.text.show.more = mehr anzeigen

journal.window.facets.curation.suggestion.display.text.this.suggestion.or.one.of.its = dieser Vorschlag oder einer seiner Treffer ist nicht mehr verfügbar. aktualisiere die Vorschläge.

journal.window.facets.curation.suggestions.accept = annehmen

journal.window.facets.curation.suggestions.account.attributed.agent = deine Schilderung, wie { $agent_name } sie geschrieben hat

journal.window.facets.curation.suggestions.basis.agent.journal.shows = { $agent_name } sagt, das in deinem Journal bemerkt zu haben

journal.window.facets.curation.suggestions.basis.agent.my.own.work = { $agent_name } sagt, das aus erster Hand zu wissen

journal.window.facets.curation.suggestions.basis.agent.outside.source = { $agent_name } sagt, das außerhalb deines Journals gefunden zu haben

journal.window.facets.curation.suggestions.basis.agent.owner.said = { $agent_name } sagt, dass du das mitgeteilt hast

journal.window.facets.curation.suggestions.cancel = abbrechen

journal.window.facets.curation.suggestions.dismiss = verwerfen

journal.window.facets.curation.suggestions.earlier.accepted = du hast das angenommen

journal.window.facets.curation.suggestions.earlier.agent = ein früherer Agent

journal.window.facets.curation.suggestions.earlier.dismissed = du hast das verworfen

journal.window.facets.curation.suggestions.earlier.in.duplicates = bei möglichen Duplikaten

journal.window.facets.curation.suggestions.earlier.not.accepted = nicht angenommen

journal.window.facets.curation.suggestions.earlier.stale = schon geändert

journal.window.facets.curation.suggestions.edit = bearbeiten

journal.window.facets.curation.suggestions.edit.name.label = Name

journal.window.facets.curation.suggestions.filter.empty.agent = von { $agent_name } wartet nichts.

journal.window.facets.curation.suggestions.filter.only.agent = nur von { $agent_name }.

journal.window.facets.curation.suggestions.group = von deinen Agenten

journal.window.facets.curation.suggestions.keep.name = zu verwendender Name

journal.window.facets.curation.suggestions.kind.name = ein Name

journal.window.facets.curation.suggestions.kind.same.person = dieselbe Person

journal.window.facets.curation.suggestions.kind.what.happened = was tatsächlich passiert ist

journal.window.facets.curation.suggestions.kind.who.said.it = wer es gesagt hat

journal.window.facets.curation.suggestions.lines.assigned.named = { $quantity ->
    [one] diese Zeile wurde von { $speaker } gesagt
   *[other] diese { $count } Zeilen wurden von { $speaker } gesagt
}

journal.window.facets.curation.suggestions.lines.assigned.owner = { $quantity ->
    [one] diese Zeile wurde von { $speaker_start }dir{ $speaker_end } gesagt
   *[other] diese { $count } Zeilen wurden von { $speaker_start }dir{ $speaker_end } gesagt
}

journal.window.facets.curation.suggestions.lines.assigned.unknown = { $quantity ->
    [one] diese Zeile wurde von { $speaker_start }einer Person, die du nicht kennst{ $speaker_end } gesagt
   *[other] diese { $count } Zeilen wurden von { $speaker_start }einer Person, die du nicht kennst{ $speaker_end } gesagt
}

journal.window.facets.curation.suggestions.name.alias = { $former_name }, auch bekannt als { $new_name }

journal.window.facets.curation.suggestions.name.change = { $former_name } → { $new_name }

journal.window.facets.curation.suggestions.now = dein Journal sagt

journal.window.facets.curation.suggestions.now.people = dein Journal hat

journal.window.facets.curation.suggestions.ok = ok

journal.window.facets.curation.suggestions.open = öffnen

journal.window.facets.curation.suggestions.open.activity = Aktivität öffnen

journal.window.facets.curation.suggestions.open.duplicates = mögliche Duplikate öffnen

journal.window.facets.curation.suggestions.open.lines = Zeilen öffnen

journal.window.facets.curation.suggestions.open.person.name = { $person_name } öffnen

journal.window.facets.curation.suggestions.person.facet.membership = in { $facet_list }

journal.window.facets.curation.suggestions.person.notes.count = { $quantity ->
    [one] 1 Notiz
   *[other] { $count } Notizen
}

journal.window.facets.curation.suggestions.receipt.accepted.alias = Vorschlag von { $agent_name } angenommen. { $former_name } heißt jetzt auch { $new_name }.

journal.window.facets.curation.suggestions.receipt.accepted.ask = Vorschlag von { $agent_name } angenommen. dein Journal wird noch einmal darüber nachdenken.

journal.window.facets.curation.suggestions.receipt.accepted.ask.unconfirmed = Vorschlag von { $agent_name } angenommen und gespeichert, aber dein Journal konnte nicht bestätigen, dass es noch einmal darüber nachdenken wird.

journal.window.facets.curation.suggestions.receipt.accepted.merged.people = Vorschlag von { $agent_name } angenommen. { $merged_name } und { $kept_name } sind jetzt eine Person, genannt { $kept_name }.

journal.window.facets.curation.suggestions.receipt.accepted.partial = Vorschlag von { $agent_name } für diese Aktivität angenommen. einige andere Aktivitäten konnten nicht aktualisiert werden.

journal.window.facets.curation.suggestions.receipt.accepted.rename = Vorschlag von { $agent_name } angenommen. { $former_name } heißt jetzt { $new_name }.

journal.window.facets.curation.suggestions.receipt.accepted.simple = Vorschlag von { $agent_name } angenommen.

journal.window.facets.curation.suggestions.receipt.dismissed.agent = verworfen. schlägt { $agent_name } das noch einmal vor, wird { $agent_name } gesagt, dass du schon nein gesagt hast.

journal.window.facets.curation.suggestions.receipt.undone.alias = rückgängig gemacht. { $new_name } ist nicht mehr ein weiterer Name für { $former_name }.

journal.window.facets.curation.suggestions.receipt.undone.rename = rückgängig gemacht. { $new_name } ist wieder { $former_name }.

journal.window.facets.curation.suggestions.retry = erneut versuchen

journal.window.facets.curation.suggestions.show.all = jeden Agenten anzeigen

journal.window.facets.curation.suggestions.show.more = mehr anzeigen

journal.window.facets.curation.suggestions.stale.changed.after.agent = das hat sich geändert, nachdem { $agent_name } es vorgeschlagen hat, also gibt es nichts anzunehmen. so steht es jetzt:

journal.window.facets.curation.suggestions.stale.chip = schon geändert

journal.window.facets.curation.suggestions.stale.pair.changed.after.agent = { $first_name } oder { $second_name } hat sich geändert, nachdem { $agent_name } das vorgeschlagen hat, also gibt es nichts anzunehmen.

journal.window.facets.curation.suggestions.stale.pair.journal.candidate = dein Journal fragt schon, ob { $first_name } und { $second_name } dieselbe Person sind. du kannst das bei möglichen Duplikaten beantworten.

journal.window.facets.curation.suggestions.stale.pair.journal.question = dein Journal hat eine eigene Frage zu { $first_name } und { $second_name }, also gibt es hier nichts anzunehmen.

journal.window.facets.curation.suggestions.stale.pair.owner.answered = du hast { $first_name } und { $second_name } schon getrennt behalten, also gibt es nichts anzunehmen.

journal.window.facets.curation.suggestions.suggested = vorgeschlagen

journal.window.facets.curation.suggestions.then = wenn du annimmst

journal.window.facets.curation.suggestions.then.account.days = dein Journal denkt noch einmal über diese Aktivität nach, mit deiner Schilderung und über { $affected_days }.

journal.window.facets.curation.suggestions.then.activities.days = { $quantity ->
    [one] dein Journal denkt noch einmal über diese Aktivität nach, und über { $affected_days }.
   *[other] dein Journal denkt noch einmal über diese { $count } Aktivitäten nach, und über { $affected_days }.
}

journal.window.facets.curation.suggestions.then.alias.name = dein Journal nennt diese Person auch { $new_name }. die frühere Version bleibt im Versionsverlauf, wo du sie wiederherstellen kannst.

journal.window.facets.curation.suggestions.then.merge.people = dein Journal behält eine Person namens { $kept_name }, auch { $merged_name } genannt, und verschiebt alles, was an { $merged_name } angehängt ist, auf sie. eine Zusammenführung lässt sich nicht rückgängig machen.

journal.window.facets.curation.suggestions.then.retain.previous.name = dein Journal nennt diese Person von jetzt an { $new_name }. { $former_name } bleibt im Versionsverlauf und lässt sich von dir wiederherstellen.

journal.window.facets.curation.suggestions.undo = rückgängig machen

journal.window.facets.curation.suggestions.undone.plain = rückgängig gemacht.

journal.window.facets.curation.suggestions.updating = wird aktualisiert…

journal.window.facets.curation.suggestions.why = warum

journal.window.facets.curation.text.loading.suggestions = Vorschläge werden geladen…

journal.window.facets.curation.visible.text.cluster = Gruppe:

journal.window.facets.curation.visible.text.could.not.load.suggestions = Vorschläge konnten nicht geladen werden.

journal.window.facets.curation.visible.text.day = Tag:

journal.window.facets.curation.visible.text.loading.suggestions = Vorschläge werden geladen…

journal.window.facets.curation.visible.text.segment = Segment:

journal.window.facets.curation.visible.text.source = Quelle:

journal.window.facets.curation.visible.text.stream = Datenstrom:

journal.window.facets.curation.window.explanation.a.close.spelling.match.was.chosen = eine enge Übereinstimmung der Schreibweise wurde automatisch gewählt

journal.window.facets.curation.window.explanation.couldn.t.accept.that.so.nothing = das konnte nicht angenommen werden, also hat sich nichts geändert. versuch es in einem Moment noch einmal.

journal.window.facets.curation.window.explanation.couldn.t.load.what.your.agents = was deine Agenten vorgeschlagen haben, konnte nicht geladen werden.

journal.window.facets.curation.window.explanation.couldn.t.undo.that.because.you = das konnte nicht rückgängig gemacht werden, weil du es seitdem geändert hast. deine Änderung bleibt.

journal.window.facets.curation.window.explanation.earlier.from.your.agents = früher von deinen Agenten

journal.window.facets.curation.window.explanation.merge.them.into.one.person = sie zu einer Person zusammenführen

journal.window.facets.curation.window.explanation.possible.duplicate.speaker.names = mögliche doppelte Sprechernamen

journal.window.facets.curation.window.explanation.saved.for.some.activities = für einige Aktivitäten gespeichert

journal.window.facets.curation.window.explanation.that.didn.t.go.through.try = das ist nicht durchgegangen. versuch es noch einmal.

journal.window.facets.curation.window.explanation.the.names.have.little.in.common = die Namen haben wenig gemeinsam

journal.window.facets.curation.window.explanation.the.suggested.entity.is.no.longer = die vorgeschlagene Entität ist nicht mehr hier

journal.window.facets.curation.window.explanation.the.suggested.entity.is.not.a = die vorgeschlagene Entität ist keine Person

journal.window.facets.curation.window.explanation.this.can.t.be.accepted.right = das kann gerade nicht angenommen werden.

journal.window.facets.curation.window.explanation.this.was.a.recording.or.video = das war eine laufende Aufnahme oder ein laufendes Video

journal.window.facets.curation.window.explanation.undone.but.your.journal.couldn.t = rückgängig gemacht, aber dein Journal konnte nicht bestätigen, dass es noch einmal darüber nachdenken wird.

journal.window.facets.curation.window.explanation.undone.your.journal.will.think.about = rückgängig gemacht. dein Journal wird noch einmal darüber nachdenken, ohne diese Änderung.

journal.window.facets.curation.window.explanation.what.you.accept.is.kept.as = was du annimmst, wird als deine eigene Schilderung behalten.

journal.window.facets.curation.window.explanation.you.accepted.it.then.undid.it = du hast das angenommen und dann rückgängig gemacht

journal.window.facets.curation.window.explanation.you.kept.them.separate = du hast sie getrennt behalten

journal.window.facets.curation.window.explanation.your.account.of.it = deine Schilderung davon

journal.window.facets.curation.window.explanation.your.journal.didn.t.answer.so = dein Journal hat nicht geantwortet, also ist das vielleicht schon erledigt. noch einmal versuchen erledigt es nicht ein zweites Mal.

journal.window.facets.curation.window.explanation.your.journal.didn.t.answer.so.variant2 = dein Journal hat nicht geantwortet, also ist das vielleicht schon angenommen. noch einmal versuchen nimmt es nicht ein zweites Mal an.

journal.window.facets.curation.window.explanation.your.journal.will.think.this.through = dein Journal wird das noch einmal durchdenken, als etwas, das du angesehen oder angehört hast, ohne dass jemand sonst beteiligt ist.

journal.window.location.day.area.unnamed = ohne benannte Stadt in der Nähe ({ $area })

journal.window.location.day.empty = Ort wurde behalten, aber kein Messwert konnte benannt werden.

journal.window.location.day.heading = Ort am { $date }

journal.window.location.day.instant = um { $time }

journal.window.location.day.moment = { $date } um { $clock }

journal.window.location.day.nearby.list = nahe { $leading } und { $last }

journal.window.location.day.place.context = { $specific }; { $broader }

journal.window.location.day.place.inside = bei { $place } in { $inside }

journal.window.location.day.place.named = bei { $owner_name } (ein Ort, den du benannt hast); { $location }

journal.window.location.day.readings.area = { $quantity ->
    [one] Messwerte { $area }, { $when }: { $count } Messwert von { $device }.
   *[other] Messwerte { $area }, { $when }: { $count } Messwerte von { $device }.
}

journal.window.location.day.readings.area.approximate = { $quantity ->
    [one] Messwerte { $area }, { $when }: { $count } ungefährer Messwert von { $device }.
   *[other] Messwerte { $area }, { $when }: { $count } ungefähre Messwerte von { $device }.
}

journal.window.location.day.readings.unlocated = { $quantity ->
    [one] Messwerte, { $when }: { $count } Messwert von { $device }.
   *[other] Messwerte, { $when }: { $count } Messwerte von { $device }.
}

journal.window.location.day.readings.unlocated.approximate = { $quantity ->
    [one] Messwerte, { $when }: { $count } ungefährer Messwert von { $device }.
   *[other] Messwerte, { $when }: { $count } ungefähre Messwerte von { $device }.
}

journal.window.location.day.stay = ein Aufenthalt, den dein Journal aus { $count } Messwerten von { $device } { $location } ermittelt hat: erster Messwert { $first }, letzter { $last }.

journal.window.location.day.visit = ein Besuch, den { $device } gemeldet hat, { $location }: { $arrival }, { $departure }.

journal.window.map.accessible.name.edit.this.place = diesen Ort bearbeiten

journal.window.map.accessible.name.location.accuracy = Ortgenauigkeit

journal.window.map.accessible.name.map = Karte

journal.window.map.accessible.name.map.of.the.records.listed.here = Karte der hier aufgeführten Datensätze, mit Städten und Grenzen

journal.window.map.accessible.name.more.of.your.location = mehr von deinem Ort

journal.window.map.accessible.name.other.weeks.and.this.month = andere Wochen und dieser Monat

journal.window.map.accessible.name.the.day.in.order = der Tag, der Reihe nach

journal.window.map.accessible.name.the.stops.across.the.day.shown = die Halte über den Tag, gezeigt in der Liste unten

journal.window.map.accessible.name.visits.and.stays.across.the.day = Besuche und Aufenthalte über den Tag

journal.window.map.accessible.name.zoom = Zoom

journal.window.map.accessible.name.zoom.in = vergrößern

journal.window.map.accessible.name.zoom.out = verkleinern

journal.window.map.accuracy.download.button = { $size } herunterladen

journal.window.map.accuracy.download.explanation = es ist{ $slot1 }{ $size }{ $slot2 }von { $slot3 }, derselbe Download für alle, also sagt das Holen nichts darüber, wo du gewesen bist. Namen werden auf deinem Computer nachgeschlagen.

journal.window.map.accuracy.download.progress = Ortgenauigkeit wird heruntergeladen:{ $slot1 }{ $received }{ $slot2 }von{ $slot3 }{ $total }. du kannst dein Journal weiter nutzen, während sie heruntergeladen wird.

journal.window.map.accuracy.explanation.names = ein Besuch, den dein Telefon meldet, wird nach den Orten innerhalb der Fehlerspanne deines Telefons benannt: "nahe { $slot1 }".

journal.window.map.accuracy.explanation.outlines = ein Besuch, den dein Telefon meldet, wird nach dem Gebäude, dem Park oder dem Campus benannt, in dem er war, und nach den Orten innerhalb der Fehlerspanne deines Telefons: "bei { $slot1 }", "nahe { $slot2 }".

journal.window.map.accuracy.offered.names = mit Ortgenauigkeit wird ein Besuch, den dein Telefon meldet, nach den Orten innerhalb der Fehlerspanne deines Telefons benannt: "nahe { $slot1 }".

journal.window.map.accuracy.offered.outlines = mit Ortgenauigkeit wird ein Besuch, den dein Telefon meldet, nach dem Gebäude, dem Park oder dem Campus benannt, in dem er war, und nach den Orten innerhalb der Fehlerspanne deines Telefons: "bei { $slot1 }", "nahe { $slot2 }".

journal.window.map.accuracy.remove.effect = gibt { $storage } frei; die Namen gehen zurück zur nahen Stadt.

journal.window.map.accuracy.status.verification.unavailable = installierte Pakete der Ortgenauigkeit konnten nicht überprüft werden

journal.window.map.accuracy.storage.insufficient = es ist nicht genug Platz frei: Ortgenauigkeit braucht { $required_storage } frei, plus etwas Reserve.

journal.window.map.accuracy.update.available = Ortgenauigkeit ist an. ein neuerer Download ist bereit von updates.solstone.app: { $size }.

journal.window.map.area.label.at.place = bei{ $slot1 }{ $name }

journal.window.map.area.label.inside = in{ $slot1 }{ $place }

journal.window.map.area.label.inside.qualified = in{ $slot1 }{ $place }{ $slot2 }{ $qualifier }

journal.window.map.area.label.near = nahe{ $slot1 }{ $place }{ $slot2 }{ $qualifier }

journal.window.map.area.label.near.place = nahe{ $slot1 }{ $name }

journal.window.map.area.label.qualified = { $region }{ $slot1 }{ $country }

journal.window.map.area.no.town = ein Ort ohne benannte Stadt in der Nähe

journal.window.map.area.size = etwa{ $slot1 }{ $size }{ $slot2 }km

journal.window.map.attribution.base = Städte von { $geonames_start }GeoNames{ $geonames_end } ({ $licence_start }CC BY 4.0{ $licence_end }) · Grenzen von Natural Earth

journal.window.map.attribution.outlines = Städte von { $geonames_start }GeoNames{ $geonames_end } ({ $licence_start }CC BY 4.0{ $licence_end }) · Grenzen von Natural Earth · Orte und Umrisse von { $overture_start }Overture Maps{ $overture_end }

journal.window.map.attribution.places = Städte von { $geonames_start }GeoNames{ $geonames_end } ({ $licence_start }CC BY 4.0{ $licence_end }) · Grenzen von Natural Earth · Orte von { $overture_start }Overture Maps{ $overture_end }

journal.window.map.button.label.clear = leeren

journal.window.map.button.label.download.again = erneut herunterladen

journal.window.map.button.label.fit = einpassen

journal.window.map.button.label.remove.location.accuracy = Ortgenauigkeit entfernen

journal.window.map.button.label.save = speichern

journal.window.map.button.label.show.on.the.map = auf der Karte anzeigen

journal.window.map.button.label.update = aktualisieren

journal.window.map.calendar.day.name = { $state ->
    [stops] { $date }, Halte
    [elsewhere] { $date }, Halte anderswo: { $places }
    [no-stops] { $date }, keine Halte
    [no-location] { $date }, kein Ort behalten
    [unreadable] { $date }, Ort konnte nicht gelesen werden
    [kept] { $date }, Ort behalten
   *[unknown] { $date }, { $state_label }
}

journal.window.map.calendar.day.name.selected = { $state ->
    [stops] { $date }, Halte, hat einen Halt im gewählten Bereich
    [elsewhere] { $date }, Halte anderswo: { $places }, hat einen Halt im gewählten Bereich
    [no-stops] { $date }, keine Halte, hat einen Halt im gewählten Bereich
    [no-location] { $date }, kein Ort behalten, hat einen Halt im gewählten Bereich
    [unreadable] { $date }, Ort konnte nicht gelesen werden, hat einen Halt im gewählten Bereich
    [kept] { $date }, Ort behalten, hat einen Halt im gewählten Bereich
   *[unknown] { $date }, { $state_label }, hat einen Halt im gewählten Bereich
}

journal.window.map.calendar.group.accessible.name = Tage im{ $slot1 }{ $month }. nutze die Pfeiltasten, um zwischen den Tagen zu wechseln.

journal.window.map.calendar.location.kept = Ort behalten

journal.window.map.calendar.location.none = kein Ort behalten

journal.window.map.calendar.stops.elsewhere = Halte anderswo

journal.window.map.calendar.stops.here = Halte hier

journal.window.map.calendar.stops.none = keine Halte

journal.window.map.count.area = { $quantity ->
    [one] { $count } Bereich
   *[other] { $count } Bereiche
}

journal.window.map.count.day = { $quantity ->
        [one] { $count } Tag
       *[other] { $count } Tage
    }

journal.window.map.count.night = { $quantity ->
    [one] { $count } Nacht
   *[other] { $count } Nächte
}

journal.window.map.count.reading = { $quantity ->
    [one] { $count } Messwert
   *[other] { $count } Messwerte
}

journal.window.map.count.record = { $quantity ->
    [one] { $count } Datensatz
   *[other] { $count } Datensätze
}

journal.window.map.count.visit = { $quantity ->
    [one] { $count } Besuch
   *[other] { $count } Besuche
}

journal.window.map.date.long = { $weekday },{ $slot1 }{ $day }.{ $slot2 }{ $month }

journal.window.map.date.short = { $day }.{ $slot1 }{ $month }

journal.window.map.date.short.weekday = { $weekday }{ $slot1 }{ $date }

journal.window.map.date.short.weekday.comma = { $weekday },{ $slot1 }{ $date }

journal.window.map.date.time = { $day }{ $slot1 }{ $clock }

journal.window.map.date.time.range = { $first }{ $slot1 }bis{ $slot2 }{ $last }

journal.window.map.day.location.none = dein Journal hat keinen Ort von { $date }. der Ort kann pausiert oder aus gewesen sein, oder das Telefon kann dein Journal nicht erreicht haben. dein Journal kann nicht sagen, was davon zutrifft.

journal.window.map.device.android.phone = { $slot1 }-Telefon

journal.window.map.device.android.tablet = { $slot1 }-Tablet

journal.window.map.device.detail.tier.balanced = Detailgrad in der solstone-App: { $level_start }Orte + Kommen und Gehen{ $level_end }.

journal.window.map.device.detail.tier.full = Detailgrad in der solstone-App: { $level_start }das vollständige Bild{ $level_end }.

journal.window.map.device.detail.tier.light = Detailgrad in der solstone-App: { $level_start }nur Orte{ $level_end }.

journal.window.map.device.from = von{ $slot1 }{ $device }

journal.window.map.device.from.android.phone = von deinem Android-Telefon

journal.window.map.device.from.android.phone.named = von deinem Android-Telefon ({ $device_name })

journal.window.map.device.from.android.tablet = von deinem Android-Tablet

journal.window.map.device.from.android.tablet.named = von deinem Android-Tablet ({ $device_name })

journal.window.map.device.from.ipad = von deinem iPad

journal.window.map.device.from.ipad.named = von deinem iPad ({ $device_name })

journal.window.map.device.from.iphone = von deinem iPhone

journal.window.map.device.from.iphone.named = von deinem iPhone ({ $device_name })

journal.window.map.device.from.phone = von deinem Telefon

journal.window.map.device.from.phone.named = von deinem Telefon ({ $device_name })

journal.window.map.device.from.tablet = von deinem Tablet

journal.window.map.device.from.tablet.named = von deinem Tablet ({ $device_name })

journal.window.map.device.from.watch = von deiner Uhr

journal.window.map.device.from.watch.named = von deiner Uhr ({ $device_name })

journal.window.map.device.interrupted.android.phone = { $quantity ->
    [one] dein Android-Telefon hat gesagt, dass der Ort einmal an diesem Tag unterbrochen wurde.
   *[other] dein Android-Telefon hat gesagt, dass der Ort { $count }-mal an diesem Tag unterbrochen wurde.
}

journal.window.map.device.interrupted.android.phone.twice = dein Android-Telefon hat gesagt, dass der Ort an diesem Tag zweimal unterbrochen wurde.

journal.window.map.device.interrupted.android.tablet = { $quantity ->
    [one] dein Android-Tablet hat gesagt, dass der Ort einmal an diesem Tag unterbrochen wurde.
   *[other] dein Android-Tablet hat gesagt, dass der Ort { $count }-mal an diesem Tag unterbrochen wurde.
}

journal.window.map.device.interrupted.android.tablet.twice = dein Android-Tablet hat gesagt, dass der Ort an diesem Tag zweimal unterbrochen wurde.

journal.window.map.device.interrupted.ipad = { $quantity ->
    [one] dein iPad hat gesagt, dass der Ort einmal an diesem Tag unterbrochen wurde.
   *[other] dein iPad hat gesagt, dass der Ort { $count }-mal an diesem Tag unterbrochen wurde.
}

journal.window.map.device.interrupted.ipad.twice = dein iPad hat gesagt, dass der Ort an diesem Tag zweimal unterbrochen wurde.

journal.window.map.device.interrupted.iphone = { $quantity ->
    [one] dein iPhone hat gesagt, dass der Ort einmal an diesem Tag unterbrochen wurde.
   *[other] dein iPhone hat gesagt, dass der Ort { $count }-mal an diesem Tag unterbrochen wurde.
}

journal.window.map.device.interrupted.iphone.twice = dein iPhone hat gesagt, dass der Ort an diesem Tag zweimal unterbrochen wurde.

journal.window.map.device.interrupted.phone = { $quantity ->
    [one] dein Telefon hat gesagt, dass der Ort einmal an diesem Tag unterbrochen wurde.
   *[other] dein Telefon hat gesagt, dass der Ort { $count }-mal an diesem Tag unterbrochen wurde.
}

journal.window.map.device.interrupted.phone.twice = dein Telefon hat gesagt, dass der Ort an diesem Tag zweimal unterbrochen wurde.

journal.window.map.device.interrupted.tablet = { $quantity ->
    [one] dein Tablet hat gesagt, dass der Ort einmal an diesem Tag unterbrochen wurde.
   *[other] dein Tablet hat gesagt, dass der Ort { $count }-mal an diesem Tag unterbrochen wurde.
}

journal.window.map.device.interrupted.tablet.twice = dein Tablet hat gesagt, dass der Ort an diesem Tag zweimal unterbrochen wurde.

journal.window.map.device.interrupted.unknown = { $quantity ->
    [one] dein { $device } hat gesagt, dass der Ort einmal an diesem Tag unterbrochen wurde.
   *[other] dein { $device } hat gesagt, dass der Ort { $count }-mal an diesem Tag unterbrochen wurde.
}

journal.window.map.device.interrupted.unknown.twice = dein { $device } hat gesagt, dass der Ort an diesem Tag zweimal unterbrochen wurde.

journal.window.map.device.interrupted.watch = { $quantity ->
    [one] deine Uhr hat gesagt, dass der Ort einmal an diesem Tag unterbrochen wurde.
   *[other] deine Uhr hat gesagt, dass der Ort { $count }-mal an diesem Tag unterbrochen wurde.
}

journal.window.map.device.interrupted.watch.twice = deine Uhr hat gesagt, dass der Ort an diesem Tag zweimal unterbrochen wurde.

journal.window.map.device.phone = Telefon

journal.window.map.device.possessive = dein{ $slot1 }{ $device }

journal.window.map.device.possessive.named = dein{ $slot1 }{ $device }{ $slot2 }({ $name })

journal.window.map.device.precision.off = genauer Ort ist für solstone auf deinem { $device } aus, also nennen seine Messwerte die Stadt, nicht das Gebäude.

journal.window.map.device.precision.off.android.phone = genauer Ort ist für solstone auf deinem Android-Telefon aus, also nennen seine Messwerte die Stadt, nicht das Gebäude.

journal.window.map.device.precision.off.android.tablet = genauer Ort ist für solstone auf deinem Android-Tablet aus, also nennen seine Messwerte die Stadt, nicht das Gebäude.

journal.window.map.device.precision.off.ipad = genauer Ort ist für solstone auf deinem iPad aus, also nennen seine Messwerte die Stadt, nicht das Gebäude.

journal.window.map.device.precision.off.iphone = genauer Ort ist für solstone auf deinem iPhone aus, also nennen seine Messwerte die Stadt, nicht das Gebäude.

journal.window.map.device.precision.off.phone = genauer Ort ist für solstone auf deinem Telefon aus, also nennen seine Messwerte die Stadt, nicht das Gebäude.

journal.window.map.device.precision.off.tablet = genauer Ort ist für solstone auf deinem Tablet aus, also nennen seine Messwerte die Stadt, nicht das Gebäude.

journal.window.map.device.precision.off.watch = genauer Ort ist für solstone auf deiner Uhr aus, also nennen ihre Messwerte die Stadt, nicht das Gebäude.

journal.window.map.device.stay.android.phone = ein Aufenthalt, den dein Journal aus Messwerten von deinem Android-Telefon ermittelt hat

journal.window.map.device.stay.android.phone.named = ein Aufenthalt, den dein Journal aus Messwerten von deinem Android-Telefon ({ $device_name }) ermittelt hat

journal.window.map.device.stay.android.tablet = ein Aufenthalt, den dein Journal aus Messwerten von deinem Android-Tablet ermittelt hat

journal.window.map.device.stay.android.tablet.named = ein Aufenthalt, den dein Journal aus Messwerten von deinem Android-Tablet ({ $device_name }) ermittelt hat

journal.window.map.device.stay.ipad = ein Aufenthalt, den dein Journal aus Messwerten von deinem iPad ermittelt hat

journal.window.map.device.stay.ipad.named = ein Aufenthalt, den dein Journal aus Messwerten von deinem iPad ({ $device_name }) ermittelt hat

journal.window.map.device.stay.iphone = ein Aufenthalt, den dein Journal aus Messwerten von deinem iPhone ermittelt hat

journal.window.map.device.stay.iphone.named = ein Aufenthalt, den dein Journal aus Messwerten von deinem iPhone ({ $device_name }) ermittelt hat

journal.window.map.device.stay.phone = ein Aufenthalt, den dein Journal aus Messwerten von deinem Telefon ermittelt hat

journal.window.map.device.stay.phone.named = ein Aufenthalt, den dein Journal aus Messwerten von deinem Telefon ({ $device_name }) ermittelt hat

journal.window.map.device.stay.tablet = ein Aufenthalt, den dein Journal aus Messwerten von deinem Tablet ermittelt hat

journal.window.map.device.stay.tablet.named = ein Aufenthalt, den dein Journal aus Messwerten deines Tablets ({ $device_name }) ermittelt hat

journal.window.map.device.stay.watch = ein Aufenthalt, den dein Journal aus Messwerten deiner Uhr ermittelt hat

journal.window.map.device.stay.watch.named = ein Aufenthalt, den dein Journal aus Messwerten deiner Uhr ({ $device_name }) ermittelt hat

journal.window.map.device.tablet = Tablet

journal.window.map.device.visit.android.phone = ein Besuch, den dein Android-Telefon gemeldet hat

journal.window.map.device.visit.android.phone.named = ein Besuch, den dein Android-Telefon ({ $device_name }) gemeldet hat

journal.window.map.device.visit.android.tablet = ein Besuch, den dein Android-Tablet gemeldet hat

journal.window.map.device.visit.android.tablet.named = ein Besuch, den dein Android-Tablet ({ $device_name }) gemeldet hat

journal.window.map.device.visit.ipad = ein Besuch, den dein iPad gemeldet hat

journal.window.map.device.visit.ipad.named = ein Besuch, den dein iPad ({ $device_name }) gemeldet hat

journal.window.map.device.visit.iphone = ein Besuch, den dein iPhone gemeldet hat

journal.window.map.device.visit.iphone.named = ein Besuch, den dein iPhone ({ $device_name }) gemeldet hat

journal.window.map.device.visit.phone = ein Besuch, den dein Telefon gemeldet hat

journal.window.map.device.visit.phone.named = ein Besuch, den dein Telefon ({ $device_name }) gemeldet hat

journal.window.map.device.visit.tablet = ein Besuch, den dein Tablet gemeldet hat

journal.window.map.device.visit.tablet.named = ein Besuch, den dein Tablet ({ $device_name }) gemeldet hat

journal.window.map.device.visit.watch = ein Besuch, den deine Uhr gemeldet hat

journal.window.map.device.visit.watch.named = ein Besuch, den deine Uhr ({ $device_name }) gemeldet hat

journal.window.map.disclosure.label.how.places.are.worked.out = wie Orte ermittelt werden

journal.window.map.disclosure.label.how.the.month.is.drawn = wie der Monat gezeichnet wird

journal.window.map.disclosure.label.how.this.is.drawn = wie das gezeichnet wird

journal.window.map.disclosure.label.licences = Lizenzen

journal.window.map.disclosure.label.why.these.words = warum diese Wörter

journal.window.map.entry.area.town.reason = { $town } ist die Stadt, die am besten für diesen Bereich steht, { $distance_km } km entfernt.

journal.window.map.entry.lines.unreadable = { $quantity ->
    [one] { $count } Zeile konnte von deinem Journal nicht gelesen werden
   *[other] { $count } Zeilen konnten von deinem Journal nicht gelesen werden
}

journal.window.map.entry.margin.within = dieser ganze Bereich liegt in { $area }.

journal.window.map.entry.open.arrival = bis { $departure }, Ankunft nicht bekannt

journal.window.map.entry.open.departure = ab { $arrival }, keine Abreise gemeldet

journal.window.map.entry.precision.approximate = ungefähr

journal.window.map.entry.readings.within = jeder Messwert hier liegt, mit seiner Fehlerspanne, in { $area }.

journal.window.map.entry.reported.visit = ein Besuch, den{ $slot1 }{ $device }{ $slot2 }gemeldet hat

journal.window.map.entry.stay.margin.inside.place = diese Messwerte liegen, mit ihren Fehlerspannen, alle innerhalb von { $margin_meters } m von ihrem Mittelpunkt, und all das liegt in { $place }.

journal.window.map.entry.stay.margin.near.place = { $place } ist { $distance_meters } m entfernt; diese Messwerte liegen, mit ihren Fehlerspannen, alle innerhalb von { $margin_meters } m von ihrem Mittelpunkt.

journal.window.map.entry.stay.margin.near.place.category = { $place } ({ $category }) ist { $distance_meters } m entfernt; diese Messwerte liegen, mit ihren Fehlerspannen, alle innerhalb von { $margin_meters } m von ihrem Mittelpunkt.

journal.window.map.entry.stay.precise.reason = { $quantity ->
    [one] { $count } genauer Messwert ist für diesen Aufenthalt aufgeführt.
   *[other] dein Journal hat diesen Aufenthalt aus { $count } genauen Messwerten ermittelt, die mindestens fünf Minuten lang innerhalb von 100 m vom ersten von ihnen geblieben sind. seine Zeiten sind die des ersten und des letzten davon.
}

journal.window.map.entry.stay.recorded.device = ein Aufenthalt, den dein Journal aus Messwerten von { $device } ermittelt hat

journal.window.map.entry.stay.within = diese Messwerte liegen, mit ihren Fehlerspannen, alle in { $area }.

journal.window.map.entry.time.arrival = angekommen{ $slot1 }{ $time }

journal.window.map.entry.time.departure = gegangen{ $slot1 }{ $time }

journal.window.map.entry.time.missing.arrival = Ankunft nicht bekannt

journal.window.map.entry.time.missing.departure = keine Abreise gemeldet

journal.window.map.entry.transcripts.segments = { $quantity ->
    [one] in Transkripten öffnen: { $count } Segment
   *[other] in Transkripten öffnen: { $count } Segmente
}

journal.window.map.entry.visit.margin.inside.place = dein Telefon hat dich innerhalb von { $margin_meters } m verortet, und all das liegt in { $place }.

journal.window.map.entry.visit.margin.near.place = { $place } ist { $distance_meters } m entfernt; dein Telefon hat dich innerhalb von { $margin_meters } m verortet.

journal.window.map.entry.visit.margin.near.place.category = { $place } ({ $category }) ist { $distance_meters } m entfernt; dein Telefon hat dich innerhalb von { $margin_meters } m verortet.

journal.window.map.entry.visit.margin.too.wide = dein Telefon hat dich innerhalb von { $margin_meters } m verortet, zu breit, um ein Gebäude oder die Orte in der Nähe zu benennen.

journal.window.map.entry.visit.within = die Position deines Telefons liegt, mit ihrer Fehlerspanne, in { $area }.

journal.window.map.field.label.call.it = nennen

journal.window.map.field.label.hide.this.place = diesen Ort ausblenden

journal.window.map.field.label.name = Name

journal.window.map.heading.couldn.t.open.this.day.s = der Ort dieses Tages konnte nicht geöffnet werden.

journal.window.map.heading.no.location.in.your.journal.yet = noch kein Ort in deinem Journal

journal.window.map.heading.no.location.kept.for.this.day = kein Ort wurde für diesen Tag behalten

journal.window.map.label.about.km = etwa 1 km

journal.window.map.leaflet.zoom.in = Hineinzoomen

journal.window.map.leaflet.zoom.out = Herauszoomen

journal.window.map.link.label.next.month = nächster Monat

journal.window.map.link.label.open.in.transcripts = in Transkripten öffnen

journal.window.map.link.label.open.the.day = den Tag öffnen

journal.window.map.link.label.place = Ort

journal.window.map.link.label.places = Orte

journal.window.map.link.label.previous.month = vorheriger Monat

journal.window.map.link.label.the.day = der Tag

journal.window.map.link.label.the.week.after = die Woche danach

journal.window.map.link.label.the.week.before = die Woche davor

journal.window.map.link.label.this.month = dieser Monat

journal.window.map.link.label.this.week = diese Woche

journal.window.map.list.conjunction = { $leading }{ $slot1 }und{ $slot2 }{ $last }

journal.window.map.month.area.around = rund um{ $slot1 }{ $name }

journal.window.map.month.area.clipped = nur der Teil innerhalb von { $month } zählt.

journal.window.map.month.area.days.in.month = { $quantity ->
    [one] { $count } Tag in diesem Monat
   *[other] { $count } Tage in diesem Monat
}

journal.window.map.month.area.days.ratio = { $days_with_stops } von { $days_in_month }

journal.window.map.month.area.days.show.all = { $quantity ->
    [one] { $count } Tag anzeigen
   *[other] alle { $count } Tage anzeigen
}

journal.window.map.month.area.distance.first = { $direction ->
   *[east] { $distance } km östlich des ersten aufgeführten Bereichs
    [north-east] { $distance } km nordöstlich des ersten aufgeführten Bereichs
    [north] { $distance } km nördlich des ersten aufgeführten Bereichs
    [north-west] { $distance } km nordwestlich des ersten aufgeführten Bereichs
    [west] { $distance } km westlich des ersten aufgeführten Bereichs
    [south-west] { $distance } km südwestlich des ersten aufgeführten Bereichs
    [south] { $distance } km südlich des ersten aufgeführten Bereichs
    [south-east] { $distance } km südöstlich des ersten aufgeführten Bereichs
}

journal.window.map.month.area.distance.month = { $direction ->
   *[east] { $distance } km östlich des meistgenutzten Bereichs
    [north-east] { $distance } km nordöstlich des meistgenutzten Bereichs
    [north] { $distance } km nördlich des meistgenutzten Bereichs
    [north-west] { $distance } km nordwestlich des meistgenutzten Bereichs
    [west] { $distance } km westlich des meistgenutzten Bereichs
    [south-west] { $distance } km südwestlich des meistgenutzten Bereichs
    [south] { $distance } km südlich des meistgenutzten Bereichs
    [south-east] { $distance } km südöstlich des meistgenutzten Bereichs
}

journal.window.map.month.area.distance.trip = { $direction ->
   *[east] { $distance } km östlich des meistgenutzten Bereichs dieser Reise
    [north-east] { $distance } km nordöstlich des meistgenutzten Bereichs dieser Reise
    [north] { $distance } km nördlich des meistgenutzten Bereichs dieser Reise
    [north-west] { $distance } km nordwestlich des meistgenutzten Bereichs dieser Reise
    [west] { $distance } km westlich des meistgenutzten Bereichs dieser Reise
    [south-west] { $distance } km südwestlich des meistgenutzten Bereichs dieser Reise
    [south] { $distance } km südlich des meistgenutzten Bereichs dieser Reise
    [south-east] { $distance } km südöstlich des meistgenutzten Bereichs dieser Reise
}

journal.window.map.month.area.live.days = { $quantity ->
    [one] { $name }, { $count } Tag in diesem Monat
   *[other] { $name }, { $count } Tage in diesem Monat
}

journal.window.map.month.area.open.stops = { $quantity ->
    [one] bei 1 Besuch fehlt die Ankunft oder die Abreise, deshalb ist seine Zeit nicht in den Stunden oder den Nächten. sein Tag zählt trotzdem.
   *[other] bei { $count } Besuchen fehlt eine Ankunft oder eine Abreise, deshalb ist ihre Zeit nicht in den Stunden oder den Nächten. ihre Tage zählen trotzdem.
}

journal.window.map.month.area.size = ein Bereich { $size } breit

journal.window.map.month.areas.list.region = { $quantity ->
    [one] { $count } Bereich { $area }, die meisten Tage zuerst.
   *[other] { $count } Bereiche { $area }, die meisten Tage zuerst.
}

journal.window.map.month.areas.list.region.home = { $quantity ->
    [one] { $count } Bereich um { $area }, die meisten Tage zuerst.
   *[other] { $count } Bereiche um { $area }, die meisten Tage zuerst.
}

journal.window.map.month.areas.list.region.shared = { $quantity ->
    [one] { $count } Bereich { $area }, alle in { $region }, die meisten Tage zuerst.
   *[other] { $count } Bereiche { $area }, alle in { $region }, die meisten Tage zuerst.
}

journal.window.map.month.areas.list.region.shared.home = { $quantity ->
    [one] { $count } Bereich um { $area }, alle in { $region }, die meisten Tage zuerst.
   *[other] { $count } Bereiche um { $area }, alle in { $region }, die meisten Tage zuerst.
}

journal.window.map.month.areas.list.trip = { $quantity ->
    [one] { $count } Bereich auf der Reise vom { $date }, die meisten Tage zuerst.
   *[other] { $count } Bereiche auf der Reise vom { $date }, die meisten Tage zuerst.
}

journal.window.map.month.areas.list.trip.shared = { $quantity ->
    [one] { $count } Bereich auf der Reise vom { $date }, alle in { $region }, die meisten Tage zuerst.
   *[other] { $count } Bereiche auf der Reise vom { $date }, alle in { $region }, die meisten Tage zuerst.
}

journal.window.map.month.areas.live.zoom = { $quantity ->
    [one] auf { $count } Bereich herangezoomt
   *[other] auf { $count } Bereiche herangezoomt
}

journal.window.map.month.areas.outlined = { $quantity ->
    [one] { $count } Bereich mit Umriss
   *[other] { $count } Bereiche mit Umriss
}

journal.window.map.month.areas.outside.show.all = { $quantity ->
    [one] { $count } Bereich außerhalb der Karte · alle anzeigen
   *[other] { $count } Bereiche außerhalb der Karte · alle anzeigen
}

journal.window.map.month.areas.show.all = { $quantity ->
    [one] { $count } Bereich anzeigen
   *[other] alle { $count } Bereiche anzeigen
}

journal.window.map.month.choice.trip = auf der Reise{ $slot2 }am{ $slot1 }{ $date }

journal.window.map.month.day.location.none = dein Journal hat keinen Ort von{ $slot1 }{ $date }. der Ort war vielleicht pausiert oder aus, oder das Telefon erreichte dein Journal vielleicht nicht. dein Journal kann nicht sagen, welches.

journal.window.map.month.day.stay.readings = { $quantity ->
    [one] { $count } Messwert von deinem Telefon ist für diesen Aufenthalt aufgeführt
   *[other] ein Aufenthalt, den dein Journal aus { $count } der Messwerte deines Telefons ermittelt hat
}

journal.window.map.month.days.show.fewer = weniger Tage anzeigen

journal.window.map.month.explanation.midnight = nur Zeit innerhalb von{ $slot1 }{ $month }{ $slot2 }zählt. ein Besuch, der am{ $slot3 }{ $previous }{ $slot4 }{ $day }{ $slot5 }begann, zählt ab Mitternacht.

journal.window.map.month.heading = { $month }{ $slot1 }{ $year }

journal.window.map.month.live.areas.all = alle Bereiche werden angezeigt

journal.window.map.month.live.choice.none = nichts gewählt

journal.window.map.month.live.fit = Karte eingepasst

journal.window.map.month.live.showing.choice = wird angezeigt{ $slot1 }{ $name }

journal.window.map.month.live.showing.home = rund um { $area } wird angezeigt

journal.window.map.month.live.zoom.amount = gezoomt{ $slot1 }{ $zoom }

journal.window.map.month.location.gaps = { $quantity ->
    [one] an { $count } Tag wurde kein Ort behalten.
   *[other] an { $count } Tagen wurde kein Ort behalten.
}

journal.window.map.month.map.area.summary = { $shaded_quantity ->
    [one] Karte von { $area }: { $shaded_areas } schattierter Bereich, { $size } breit. derselbe Bereich ist unter Bereiche in diesem Monat aufgeführt.
   *[other] Karte von { $area }: { $shaded_areas } schattierte Bereiche, jeweils { $size } breit. dieselben Bereiche sind unter Bereiche in diesem Monat aufgeführt.
}

journal.window.map.month.map.area.summary.home = { $shaded_quantity ->
    [one] Karte des Bereichs um { $area }: { $shaded_areas } schattierter Bereich, { $size } breit. derselbe Bereich ist unter Bereiche in diesem Monat aufgeführt.
   *[other] Karte des Bereichs um { $area }: { $shaded_areas } schattierte Bereiche, jeweils { $size } breit. dieselben Bereiche sind unter Bereiche in diesem Monat aufgeführt.
}

journal.window.map.month.map.group.summary = { $shaded_quantity ->
    [one] { $area_quantity ->
        [one] Karte von { $area }: { $shaded_areas } schattierter Bereich, { $size } breit, darin der { $areas } Bereich, der unter Bereiche in diesem Monat aufgeführt ist.
       *[other] Karte von { $area }: { $shaded_areas } schattierter Bereich, { $size } breit, darin die { $areas } Bereiche, die unter Bereiche in diesem Monat aufgeführt sind.
    }
   *[other] { $area_quantity ->
        [one] Karte von { $area }: { $shaded_areas } schattierte Bereiche, jeweils { $size } breit, darin der { $areas } Bereich, der unter Bereiche in diesem Monat aufgeführt ist.
       *[other] Karte von { $area }: { $shaded_areas } schattierte Bereiche, jeweils { $size } breit, darin die { $areas } Bereiche, die unter Bereiche in diesem Monat aufgeführt sind.
    }
}

journal.window.map.month.map.group.summary.home = { $shaded_quantity ->
    [one] { $area_quantity ->
        [one] Karte des Bereichs um { $area }: { $shaded_areas } schattierter Bereich, { $size } breit, darin der { $areas } Bereich, der unter Bereiche in diesem Monat aufgeführt ist.
       *[other] Karte des Bereichs um { $area }: { $shaded_areas } schattierter Bereich, { $size } breit, darin die { $areas } Bereiche, die unter Bereiche in diesem Monat aufgeführt sind.
    }
   *[other] { $area_quantity ->
        [one] Karte des Bereichs um { $area }: { $shaded_areas } schattierte Bereiche, jeweils { $size } breit, darin der { $areas } Bereich, der unter Bereiche in diesem Monat aufgeführt ist.
       *[other] Karte des Bereichs um { $area }: { $shaded_areas } schattierte Bereiche, jeweils { $size } breit, darin die { $areas } Bereiche, die unter Bereiche in diesem Monat aufgeführt sind.
    }
}

journal.window.map.month.map.nothing.drawn = hier nichts gezeichnet

journal.window.map.month.readings.undrawn = { $quantity ->
    [one] nicht gezeichnet: { $count } Messwert.
   *[other] nicht gezeichnet: { $count } Messwerte.
}

journal.window.map.month.stops.calendar = { $stops_quantity ->
    [one] { $days_with_stops } von { $days_in_month } Tagen hat Halte
   *[other] { $days_with_stops } von { $days_in_month } Tagen haben Halte
}

journal.window.map.month.stops.glance = { $stops_quantity ->
    [one] { $days_with_stops } von { $days_in_month } Tagen hat Halte.
   *[other] { $days_with_stops } von { $days_in_month } Tagen haben Halte.
}

journal.window.map.month.stops.glance.home.days = { $stops_quantity ->
    [one] { $days_with_stops } von { $days_in_month } Tagen hat Halte: { $home_days } um { $home_area }.
   *[other] { $days_with_stops } von { $days_in_month } Tagen haben Halte: { $home_days } um { $home_area }.
}

journal.window.map.month.stops.glance.trips.days = { $trip_quantity ->
    [one] { $days_with_stops } von { $days_in_month } Tagen { $stops_quantity ->
        [one] hat
       *[other] haben
    } Halte: { $home_days } um { $home_area }, und { $trips } Reise mehr als 50 km entfernt.
   *[other] { $days_with_stops } von { $days_in_month } Tagen { $stops_quantity ->
        [one] hat
       *[other] haben
    } Halte: { $home_days } um { $home_area }, und { $trips } Reisen mehr als 50 km entfernt.
}

journal.window.map.month.stops.undrawn.hidden = { $quantity ->
    [one] ein Ort, den du ausgeblendet hast
   *[other] { $count } Orte, die du ausgeblendet hast
}

journal.window.map.month.stops.undrawn.wide = { $quantity ->
    [one] { $count } Halt mit einer Fehlerspanne über 500 m oder ohne Angabe
   *[other] { $count } Halte mit einer Fehlerspanne über 500 m oder ohne Angabe
}

journal.window.map.month.strip.faded.bar = einem verblassenden Balken fehlt die Ankunft oder die Abreise.

journal.window.map.month.strip.visits.and.stays = gefüllte Balken sind Besuche, die dein Telefon gemeldet hat; umrandete Balken sind Aufenthalte, die dein Journal ermittelt hat.

journal.window.map.month.time.all.day = ganztägig

journal.window.map.month.time.from = von{ $slot1 }{ $time }

journal.window.map.month.time.until = bis{ $slot1 }{ $time }

journal.window.map.month.zoom.notice = hineingezoomt.

journal.window.map.months.label.april = April

journal.window.map.months.label.august = August

journal.window.map.months.label.december = Dezember

journal.window.map.months.label.february = Februar

journal.window.map.months.label.january = Januar

journal.window.map.months.label.july = Juli

journal.window.map.months.label.june = Juni

journal.window.map.months.label.march = März

journal.window.map.months.label.may = Mai

journal.window.map.months.label.november = November

journal.window.map.months.label.october = Oktober

journal.window.map.months.label.september = September

journal.window.map.navigation.go.day = zu { $date } gehen

journal.window.map.navigation.go.month = zu { $month } { $year } gehen

journal.window.map.option.label.home = Zuhause

journal.window.map.option.label.nothing = nichts

journal.window.map.option.label.work = Arbeit

journal.window.map.place.named.link = ein Ort, den du "{ $name }" genannt hast

journal.window.map.place.named.reason = du hast diesen Ort "{ $name }" genannt.

journal.window.map.place.recorded.dates = { $quantity ->
    [one] Besuche oder Aufenthalte hier an { $count } Datum.
   *[other] Besuche oder Aufenthalte hier an { $count } Tagen.
}

journal.window.map.place.recorded.dates.range = { $quantity ->
    [one] Besuche oder Aufenthalte hier an { $count } Datum, { $first } bis { $last }.
   *[other] Besuche oder Aufenthalte hier an { $count } Tagen. { $first } bis { $last }.
}

journal.window.map.place.recorded.dates.single = { $quantity ->
    [one] Besuche oder Aufenthalte hier an { $count } Datum, { $date }.
   *[other] Besuche oder Aufenthalte hier an { $count } Tagen. { $date }.
}

journal.window.map.place.recorded.nights = { $quantity ->
    [one] an { $count } Nacht deckt ein Besuch oder ein Aufenthalt hier mindestens zwei Stunden zwischen 22:00 und 06:00 ab.
   *[other] an { $count } Nächten deckt ein Besuch oder ein Aufenthalt hier mindestens zwei Stunden zwischen 22:00 und 06:00 ab.
}

journal.window.map.place.visibility.hidden = · ausgeblendet

journal.window.map.response.accuracy.busy = die Ortgenauigkeit wird schon heruntergeladen.

journal.window.map.response.accuracy.busy.detail = Ein Download läuft

journal.window.map.response.accuracy.download.failed = die Ortgenauigkeit konnte nicht heruntergeladen werden.

journal.window.map.response.accuracy.not.offered = diese Version deines Journals hat keinen Download für die Ortgenauigkeit.

journal.window.map.response.accuracy.not.offered.detail = Nichts angeboten

journal.window.map.response.accuracy.remove.failed = die Ortgenauigkeit konnte nicht entfernt werden.

journal.window.map.response.area.unavailable = die Namen der Orte, die mit deinem Journal kommen, konnten nicht gelesen werden.

journal.window.map.response.day.invalid = dieser Tag konnte nicht verwendet werden.

journal.window.map.response.day.invalid.detail = Tag nicht gefunden

journal.window.map.response.edit.alias = { $slot1 } muss { $slot2 }, { $slot3 } oder { $slot4 } sein

journal.window.map.response.edit.body = Der Inhalt muss ein { $slot1 }-Objekt sein

journal.window.map.response.edit.failed = diese Änderung konnte nicht gespeichert werden.

journal.window.map.response.edit.hidden = { $slot1 } muss { $slot2 } oder { $slot3 } sein

journal.window.map.response.edit.invalid = diese Änderung konnte nicht verwendet werden.

journal.window.map.response.edit.name = { $slot1 } muss eine Zeichenkette oder { $slot2 } sein

journal.window.map.response.month.invalid = dieser Monat konnte nicht verwendet werden.

journal.window.map.response.month.invalid.detail = Monat muss { $slot1 } sein

journal.window.map.response.place.unknown = dieser Ort ist nicht in deinem Journal.

journal.window.map.response.place.unknown.detail = Ort nicht gefunden

journal.window.map.section.heading.areas.this.month = Bereiche in diesem Monat

journal.window.map.section.heading.day.by.day = Tag für Tag

journal.window.map.section.heading.deleting.any.of.this = etwas davon löschen

journal.window.map.section.heading.edit.this.place = diesen Ort bearbeiten

journal.window.map.section.heading.location.accuracy = Ortgenauigkeit

journal.window.map.section.heading.the.day.in.order = der Tag, der Reihe nach

journal.window.map.section.heading.the.map = die Karte

journal.window.map.section.heading.the.month.at.a.glance = der Monat auf einen Blick

journal.window.map.section.heading.visits.and.stays.here = Besuche und Aufenthalte hier

journal.window.map.status.text.couldn.t.open.this.month.s = der Ort dieses Monats konnte nicht geöffnet werden.

journal.window.map.text.loading = wird geladen…

journal.window.map.text.loading.this.day.s.location = Ort dieses Tages wird geladen…

journal.window.map.text.loading.your.journal.s.location = Ort deines Journals wird geladen…

journal.window.map.visible.text.a.hidden.place.is.off.the = ein ausgeblendeter Ort ist nicht auf der Karte, und was dein Journal darüber schreibt, wo du jeden Tag warst, lässt ihn aus: in der Suche, für verbundene Agenten und in Aktivitäten. neue Tagesbriefings lassen ihn auch aus.

journal.window.map.visible.text.a.night.counts.in.an.area = eine Nacht zählt in einem Bereich, wenn ein Besuch dort mindestens zwei Stunden zwischen 22:00 und 06:00 umfasst.

journal.window.map.visible.text.a.place.is.a.spot.your = ein Ort ist eine Stelle, an der dein Telefon einen Besuch gemeldet hat oder an der du geblieben bist, und das an zwei verschiedenen Tagen, nah genug beieinander für eine einzige Stelle. ein ungefährer Standort wird nie zu einem Ort.

journal.window.map.visible.text.a.place.lasts.as.long.as = ein Ort bleibt so lange wie ein Segment, aus dem er entstanden ist. lösche diese Segmente in den Transkripten, dann gehen der Ort und sein Name auch.

journal.window.map.visible.text.a.place.starts.from.visits.or = ein Ort entsteht aus Besuchen oder Aufenthalten an zwei oder mehr Tagen, alle innerhalb von 50 m voneinander und mehr als 100 m von jedem anderen Ort entfernt. spätere innerhalb von 50 m von ihm kommen dazu. ein Aufenthalt oder Besuch mit einer Fehlerspanne über 50 m bildet nie einen Ort und kommt nie zu einem dazu.

journal.window.map.visible.text.a.place.you.hid.isn.t = ein Ort, den du ausgeblendet hast, wird nicht gezeichnet. seine Besuche erscheinen weiter in der Liste jedes Tages.

journal.window.map.visible.text.a.stay.is.worked.out.from = ein Aufenthalt wird aus genauen Messwerten ermittelt, die höchstens fünf Minuten auseinanderliegen und mindestens fünf Minuten lang innerhalb von 100 m vom ersten von ihnen geblieben sind. ungefähre Messwerte bilden nie einen Aufenthalt. seine Zeiten sind sein erster und sein letzter Messwert.

journal.window.map.visible.text.a.stop.at.one.of.your = ein Halt an einem deiner Orte wird am Ort gezeichnet, sodass ein Ort nie auf zwei Bereiche aufgeteilt wird. ein Halt, dessen Fehlerspanne nicht gemeldet wurde oder über 500 m liegt, wird nicht gezeichnet, auch nicht die Zeit zwischen den Halten.

journal.window.map.visible.text.also.in.this.area = auch in diesem Bereich

journal.window.map.visible.text.checking.location.accuracy.before.using.it = die Ortgenauigkeit wird geprüft, bevor du sie verwendest. du kannst dein Journal weiter nutzen.

journal.window.map.visible.text.days.here = Tage hier

journal.window.map.visible.text.days.with.a.stop = Tage mit einem Halt

journal.window.map.visible.text.each.area.is.about.km.across = jeder Bereich ist etwa 1 km breit und enthält die Halte darin: die Besuche, die dein Telefon gemeldet hat, und Aufenthalte, die dein Journal ermittelt hat.

journal.window.map.visible.text.every.record.above.opens.in.transcripts = jeder Datensatz oben öffnet sich in den Transkripten, wo du seine Segmente so löschst wie jedes andere. ein gelöschtes Segment verlässt diese Ansicht und die Suche.

journal.window.map.visible.text.filled.bars.are.visits.your.phone = gefüllte Balken sind Besuche, die dein Telefon gemeldet hat; umrandete Balken sind Aufenthalte, die dein Journal ermittelt hat. jeder Messwert steht auf der Seite jedes Tages.

journal.window.map.visible.text.for.readings.the.times.are.the = bei Messwerten sind die Zeiten der erste und der letzte Messwert in der Zeile. sie sagen nicht, wie lange du irgendwo warst.

journal.window.map.visible.text.hidden = ausgeblendet

journal.window.map.visible.text.hours.are.shown.for.one.area = Stunden werden für jeweils einen Bereich angezeigt, und nur für Zeit, deren beide Enden bekannt sind. sie werden nicht über Bereiche hinweg addiert, weil zwei Meldungen dieselbe Stunde in zwei Bereiche legen können.

journal.window.map.visible.text.hours.with.both.ends.known = Stunden, deren beide Enden bekannt sind

journal.window.map.visible.text.location.accuracy.is.on = die Ortgenauigkeit ist an.

journal.window.map.visible.text.location.accuracy.is.unavailable = die Ortgenauigkeit ist nicht verfügbar.

journal.window.map.visible.text.location.may.have.been.paused.or = der Ort war vielleicht pausiert oder aus, oder das Telefon erreichte dein Journal vielleicht nicht. dein Journal kann nicht sagen, welches.

journal.window.map.visible.text.location.was.kept.this.day.but = Ort wurde an diesem Tag behalten, aber keine Halte.

journal.window.map.visible.text.named.features.only.streets.not.shown = nur benannte Merkmale; Straßen nicht gezeigt.

journal.window.map.visible.text.nights = Nächte

journal.window.map.visible.text.no.location.kept.for.this.day = kein Ort wurde für diesen Tag behalten.

journal.window.map.visible.text.no.location.kept.for.this.month = kein Ort wurde für diesen Monat behalten.

journal.window.map.visible.text.no.places.yet = noch keine Orte.

journal.window.map.visible.text.no.stops.to.draw.this.month = in diesem Monat sind keine Halte zu zeichnen.

journal.window.map.visible.text.not.drawn.here = hier nicht gezeichnet:

journal.window.map.visible.text.nothing.drawn.here = · hier nichts gezeichnet

journal.window.map.visible.text.on.the.map.a.run.of = auf der Karte erscheint eine Folge genauer Messwerte als ein Punkt an ihrem mittleren Messwert, sodass sich ein Tag nicht als Route liest. die Liste zählt jeden Messwert.

journal.window.map.visible.text.saved = gespeichert.

journal.window.map.visible.text.shades.run.from.day.lightest.through = die Farbtöne reichen von 1 Tag, am hellsten, über 2 bis 3, 4 bis 7 und 8 bis 14 bis zu 15 oder mehr Tagen, am dunkelsten.

journal.window.map.visible.text.show.fewer = weniger anzeigen

journal.window.map.visible.text.stops = Halte

journal.window.map.visible.text.stops.in.order = Halte, der Reihe nach

journal.window.map.visible.text.that.change.couldn.t.be.saved = diese Änderung konnte nicht gespeichert werden.

journal.window.map.visible.text.the.shade.counts.days.not.hours = der Farbton zählt Tage, nicht Stunden. ein Tag zählt in einem Bereich einmal, bei beliebig vielen Meldungen, und ein Besuch zählt für seinen Tag, auch wenn Ankunft oder Abreise nicht bekannt ist.

journal.window.map.visible.text.the.stops.your.journal.holds.for = die Halte, die dein Journal für diesen Monat hält. jeder Bereich ist danach schattiert, wie viele Tage einen Halt darin haben.

journal.window.map.visible.text.there.s.no.street.map.a = es gibt keine Straßenkarte. eine Straßenkarte wird Stück für Stück von einem Kartendienst geladen, und nach dem richtigen Stück zu fragen sagt diesem Dienst, wo du warst.

journal.window.map.visible.text.there.s.no.street.map.the = es gibt keine Straßenkarte. die Namen sind die, die mit deinem Journal kamen oder die du heruntergeladen hast, und kein Kartendienst wird nach irgendetwas gefragt.

journal.window.map.visible.text.to.remove.a.visit.or.stay = um einen Besuch oder einen Aufenthalt zu entfernen, öffne seinen Tag und lösche seine Segmente in den Transkripten. ein Ort bleibt so lange wie ein Segment, aus dem er entstanden ist.

journal.window.map.visible.text.what.you.call.a.place.a = was du einen Ort nennst, ein Name oder Zuhause oder Arbeit, wird in der Suche und vom eigenen Denken deines Journals verwendet, und ein Agent, der Transkripte über dein ganzes Journal hinweg sehen darf, kann es lesen.

journal.window.map.visible.text.where = wo

journal.window.map.visible.text.where.starts.around.the.area.with = "wo" beginnt rund um den Bereich mit den meisten Tagen: jeder Halt innerhalb von 50 km davon. jede Folge von Tagen mit einem Halt weiter weg ist eine Reise, benannt nach ihren Tagen.

journal.window.map.visible.text.with.location.accuracy.a.visit.can = mit Ortgenauigkeit kann ein Besuch "nahe" zu den Orten innerhalb der Fehlerspanne deines Telefons sagen. jeder Datensatz sagt, warum er sich so liest, wie er sich liest.

journal.window.map.visible.text.with.location.accuracy.a.visit.can.variant2 = mit Ortgenauigkeit kann ein Besuch "an" einem Ort sagen, wenn die ganze Fehlerspanne deines Telefons innerhalb seines Umrisses liegt, und "nahe" bei den Orten innerhalb dieser Fehlerspanne. "in" heißt, dass die ganze Fehlerspanne innerhalb eines größeren Umrisses liegt, eines Viertels oder einer Stadt. jeder Datensatz sagt, warum er sich so liest, wie er sich liest.

journal.window.map.visible.text.without.it.a.visit.is.named = ohne sie wird ein Besuch nach der nahe gelegenen Stadt benannt.

journal.window.map.visible.text.without.location.accuracy.each.reading.is = ohne Ortgenauigkeit wird jeder Messwert nach der Stadt benannt, die am besten für seinen Bereich steht, was nicht immer die Stadt ist, in der du warst. deshalb sagt er "nahe".

journal.window.map.visible.text.you.can.name.a.place.call = du kannst einen Ort benennen, ihn Zuhause oder Arbeit nennen oder ihn ausblenden. beim Ausblenden bleibt alles innerhalb von 50 m von ihm von der Karte weg, und was dein Journal darüber schreibt, wo du jeden Tag warst, lässt ihn aus. die Liste des Tages hier zeigt ihn weiter.

journal.window.map.visible.text.your.journal.couldn.t.read.the = der für diesen Tag behaltene Ort konnte von deinem Journal nicht gelesen werden.

journal.window.map.visible.text.your.journal.draws.this.from.the = dein Journal zeichnet das aus den Messwerten, die dein Telefon ihm gesendet hat, und aus den Namen, die mit deinem Journal kamen oder die du heruntergeladen hast. kein Kartendienst wird nach irgendetwas gefragt.

journal.window.map.visible.text.your.phone.often.reports.an.arrival = dein Telefon meldet oft zuerst eine Ankunft und später den ganzen Besuch. die beiden Meldungen sind ein Besuch.

journal.window.map.week.heading = die Woche vom{ $slot1 }{ $date }

journal.window.map.week.readings.without.stops = { $quantity ->
    [one] { $count } Messwert, keine Besuche oder Aufenthalte
   *[other] { $count } Messwerte, keine Besuche oder Aufenthalte
}

journal.window.map.window.explanation.a.place.in.your.journal = ein Ort in deinem Journal

journal.window.map.window.explanation.a.segment.your.journal.couldn.t = ein Segment, das dein Journal nicht lesen konnte

journal.window.map.window.explanation.a.stay.your.journal.worked.out = ein Aufenthalt, den dein Journal ermittelt hat

journal.window.map.window.explanation.a.visit.your.phone.reported = ein Besuch, den dein Telefon gemeldet hat

journal.window.map.window.explanation.approximate.location.each.reading.is.only = ungefährer Ort: jeder Messwert gilt nur bis auf ein paar Kilometer, genug, um die Stadt zu benennen, nicht das Gebäude.

journal.window.map.window.explanation.choose.a.day.in.the.calendar = wähle einen Tag im Kalender.

journal.window.map.window.explanation.choose.an.area.on.the.map = wähle einen Bereich auf der Karte oder in der Liste, oder einen Tag im Kalender.

journal.window.map.window.explanation.each.bar.is.a.visit.your = jeder Balken ist ein Besuch, den dein Telefon gemeldet hat.

journal.window.map.window.explanation.hidden.from.the.map = von der Karte ausgeblendet

journal.window.map.window.explanation.in.the.chosen.area = im gewählten Bereich

journal.window.map.window.explanation.location.accuracy.couldn.t.be.downloaded = die Ortgenauigkeit konnte nicht heruntergeladen werden. versuch es in einer Weile noch einmal.

journal.window.map.window.explanation.location.accuracy.couldn.t.be.saved = die Ortgenauigkeit konnte nicht im Ordner deines Journals gespeichert werden.

journal.window.map.window.explanation.location.accuracy.failed.its.file.check = die Ortgenauigkeit hat die Dateiprüfung nicht bestanden. lade sie erneut herunter, bevor du sie nutzt.

journal.window.map.window.explanation.location.comes.from.the.solstone.app = der Ort kommt von der solstone-App auf deinem Telefon, sobald du ihn dort einschaltest. was sie entgegennimmt, geht in dein Journal und erscheint hier, Tag für Tag.

journal.window.map.window.explanation.location.couldn.t.be.read = Ort konnte nicht gelesen werden

journal.window.map.window.explanation.map.fitted.to.these.areas = Karte auf diese Bereiche eingepasst

journal.window.map.window.explanation.margin.of.error.over.m.or = Fehlerspanne über 500 m oder nicht gemeldet

journal.window.map.window.explanation.no.named.town.nearby = keine benannte Stadt in der Nähe

journal.window.map.window.explanation.open.it.in.transcripts.to.see = öffne es in den Transkripten, um es anzusehen oder zu löschen

journal.window.map.window.explanation.the.area.with.the.most.days = der Bereich mit den meisten Tagen

journal.window.map.window.explanation.this.trip.s.busiest.area = der geschäftigste Bereich dieser Reise

journal.window.map.window.explanation.this.version.of.your.journal.has = diese Version deines Journals hat keinen Download für die Ortgenauigkeit.

journal.window.map.window.explanation.tied.for.the.most.days = gleichauf bei den meisten Tagen

journal.window.map.window.explanation.where.two.reports.cover.the.same = wo zwei Meldungen dieselbe Zeit abdecken, zählen die Stunden diese Zeit einmal.

journal.window.map.window.explanation.your.journal.couldn.t.read.part = ein Teil des Orts dieses Tages konnte von deinem Journal nicht gelesen werden.

journal.window.map.window.explanation.your.watch.gives.only.approximate.location = deine Uhr gibt nur einen ungefähren Ort, deshalb benennen ihre Messwerte die Stadt, nicht das Gebäude.
