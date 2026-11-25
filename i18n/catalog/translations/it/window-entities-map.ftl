journal.window.entities.accessible.label.dismiss = ignora

journal.window.entities.accessible.name.close = chiudi

journal.window.entities.accessible.name.entity.folders = cartelle delle entità

journal.window.entities.accessible.name.search.detected.entities = cerca le entità rilevate

journal.window.entities.accessible.name.search.entities = cerca le entità

journal.window.entities.accessible.name.search.entity.cards = cerca le schede delle entità

journal.window.entities.accessible.name.sort.entities = ordina le entità

journal.window.entities.aliases.other.names = noto anche come: { $aliases }

journal.window.entities.button.detail.reattach = allega di nuovo

journal.window.entities.button.label.attach.to.facet = allega alla facetta

journal.window.entities.button.label.cancel = annulla

journal.window.entities.button.label.detach = separa

journal.window.entities.button.label.hide = nascondi

journal.window.entities.button.label.next = avanti

journal.window.entities.button.label.previous = precedente

journal.window.entities.button.label.save.changes = salva le modifiche

journal.window.entities.column.heading.description = descrizione

journal.window.entities.column.heading.name = nome

journal.window.entities.connections.calendar.last.day = { $day }

journal.window.entities.connections.evidence.show.more = { $shown_quantity ->
    [one] in mostra { $shown } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }. mostra altro
    [many] in mostra { $shown } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }. mostra altro
   *[other] in mostra { $shown } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }. mostra altro
    }

journal.window.entities.connections.row.attendance = { $total_quantity ->
    [one] 1 evento insieme · { $day }
    [many] { $total } di eventi insieme · { $day }
   *[other] { $total } eventi insieme · { $day }
    }

journal.window.entities.connections.row.direct = { $total_quantity ->
    [one] 1 momento · { $day }
    [many] { $total } di momenti · { $day }
   *[other] { $total } momenti · { $day }
    }

journal.window.entities.connections.summary = { $direct_quantity ->
    [one] { $direct } con prova diretta · { $shared_quantity ->
        [one] { $shared }
        [many] { $shared }
       *[other] { $shared }
    } solo da eventi condivisi
    [many] { $direct } con prova diretta · { $shared_quantity ->
        [one] { $shared }
        [many] { $shared }
       *[other] { $shared }
    } solo da eventi condivisi
   *[other] { $direct } con prova diretta · { $shared_quantity ->
        [one] { $shared }
        [many] { $shared }
       *[other] { $shared }
    } solo da eventi condivisi
    }

journal.window.entities.connections.summary.with.calendar = { $direct_quantity ->
    [one] { $direct } con prova diretta · { $shared_quantity ->
        [one] { $shared }
        [many] { $shared }
       *[other] { $shared }
    } solo da eventi condivisi · { $calendar_quantity ->
        [one] { $calendar }
        [many] { $calendar }
       *[other] { $calendar }
    } solo dal tuo calendario
    [many] { $direct } con prova diretta · { $shared_quantity ->
        [one] { $shared }
        [many] { $shared }
       *[other] { $shared }
    } solo da eventi condivisi · { $calendar_quantity ->
        [one] { $calendar }
        [many] { $calendar }
       *[other] { $calendar }
    } solo dal tuo calendario
   *[other] { $direct } con prova diretta · { $shared_quantity ->
        [one] { $shared }
        [many] { $shared }
       *[other] { $shared }
    } solo da eventi condivisi · { $calendar_quantity ->
        [one] { $calendar }
        [many] { $calendar }
       *[other] { $calendar }
    } solo dal tuo calendario
    }

journal.window.entities.connections.you.calendar.latest = solo sul tuo calendario · ultimo { $day }

journal.window.entities.connections.you.latest.attended.with = { $moments_quantity ->
    [one] 1 momento insieme · ultimi eventi ({ $day })
    [many] { $moments } di momenti insieme · ultimi eventi ({ $day })
   *[other] { $moments } momenti insieme · ultimi eventi ({ $day })
    }

journal.window.entities.connections.you.latest.co.present = { $moments_quantity ->
    [one] 1 momento insieme · ultimo in zona insieme ({ $day })
    [many] { $moments } di momenti insieme · ultimo in zona insieme ({ $day })
   *[other] { $moments } momenti insieme · ultimo in zona insieme ({ $day })
    }

journal.window.entities.connections.you.latest.created = { $moments_quantity ->
    [one] 1 momento insieme · ultimo creato ({ $day })
    [many] { $moments } di momenti insieme · ultimo creato ({ $day })
   *[other] { $moments } momenti insieme · ultimo creato ({ $day })
    }

journal.window.entities.connections.you.latest.fallback = { $moments_quantity ->
    [one] 1 momento insieme · ultimo { $evidence_name } ({ $day })
    [many] { $moments } di momenti insieme · ultimo { $evidence_name } ({ $day })
   *[other] { $moments } momenti insieme · ultimo { $evidence_name } ({ $day })
    }

journal.window.entities.connections.you.latest.family.of = { $moments_quantity ->
    [one] 1 momento insieme · ultima famiglia ({ $day })
    [many] { $moments } di momenti insieme · ultima famiglia ({ $day })
   *[other] { $moments } momenti insieme · ultima famiglia ({ $day })
    }

journal.window.entities.connections.you.latest.knows = { $moments_quantity ->
    [one] 1 momento insieme · ultimo conosce ({ $day })
    [many] { $moments } di momenti insieme · ultimo conosce ({ $day })
   *[other] { $moments } momenti insieme · ultimo conosce ({ $day })
    }

journal.window.entities.connections.you.latest.mentioned = { $moments_quantity ->
    [one] 1 momento insieme · ultima menzione ({ $day })
    [many] { $moments } di momenti insieme · ultima menzione ({ $day })
   *[other] { $moments } momenti insieme · ultima menzione ({ $day })
    }

journal.window.entities.connections.you.latest.messaged.with = { $moments_quantity ->
    [one] 1 momento insieme · ultimo messaggio ({ $day })
    [many] { $moments } di momenti insieme · ultimo messaggio ({ $day })
   *[other] { $moments } momenti insieme · ultimo messaggio ({ $day })
    }

journal.window.entities.connections.you.latest.other = { $moments_quantity ->
    [one] 1 momento insieme · ultimo in relazione ({ $day })
    [many] { $moments } di momenti insieme · ultimo in relazione ({ $day })
   *[other] { $moments } momenti insieme · ultimo in relazione ({ $day })
    }

journal.window.entities.connections.you.latest.party.of = { $moments_quantity ->
    [one] 1 momento insieme · ultimo parte in ({ $day })
    [many] { $moments } di momenti insieme · ultimo parte in ({ $day })
   *[other] { $moments } momenti insieme · ultimo parte in ({ $day })
    }

journal.window.entities.connections.you.latest.reports.to = { $moments_quantity ->
    [one] 1 momento insieme · ultimo fa capo a ({ $day })
    [many] { $moments } di momenti insieme · ultimo fa capo a ({ $day })
   *[other] { $moments } momenti insieme · ultimo fa capo a ({ $day })
    }

journal.window.entities.connections.you.latest.scheduled.with = { $moments_quantity ->
    [one] 1 momento insieme · ultimo in programma ({ $day })
    [many] { $moments } di momenti insieme · ultimo in programma ({ $day })
   *[other] { $moments } momenti insieme · ultimo in programma ({ $day })
    }

journal.window.entities.connections.you.latest.spoke.with = { $moments_quantity ->
    [one] 1 momento insieme · ultimo parlato ({ $day })
    [many] { $moments } di momenti insieme · ultimo parlato ({ $day })
   *[other] { $moments } momenti insieme · ultimo parlato ({ $day })
    }

journal.window.entities.connections.you.latest.uses = { $moments_quantity ->
    [one] 1 momento insieme · ultimo usa ({ $day })
    [many] { $moments } di momenti insieme · ultimo usa ({ $day })
   *[other] { $moments } momenti insieme · ultimo usa ({ $day })
    }

journal.window.entities.connections.you.latest.works.at = { $moments_quantity ->
    [one] 1 momento insieme · ultimo lavora presso ({ $day })
    [many] { $moments } di momenti insieme · ultimo lavora presso ({ $day })
   *[other] { $moments } momenti insieme · ultimo lavora presso ({ $day })
    }

journal.window.entities.connections.you.latest.works.with = { $moments_quantity ->
    [one] 1 momento insieme · ultimo lavora con ({ $day })
    [many] { $moments } di momenti insieme · ultimo lavora con ({ $day })
   *[other] { $moments } momenti insieme · ultimo lavora con ({ $day })
    }

journal.window.entities.copy.cards.empty = ancora nessuna entità in questa facetta. segna con una stella ciò che vuoi tenere qui sotto, o aggiungine una tu.

journal.window.entities.copy.conn.ambiguous = questa potrebbe corrispondere a più di un'entità.

journal.window.entities.copy.conn.calendar.only = solo calendario

journal.window.entities.copy.conn.empty = ancora nessuna connessione conservata. compaiono quando questa entità compare con qualcuno o qualcos'altro.

journal.window.entities.copy.conn.events.only = solo eventi

journal.window.entities.copy.conn.evidence.caption = i momenti dietro a questo: prova, non una conclusione.

journal.window.entities.copy.conn.evidence.failed = non è stato possibile caricare questi momenti. riprova.

journal.window.entities.copy.conn.evidence.more = in mostra { $slot1 } di { $slot2 }. mostra altro

journal.window.entities.copy.conn.evidence.view.entity = vedi l'entità →

journal.window.entities.copy.conn.horizon = le connessioni dal tempo nello stesso luogo risalgono solo fino a { $slot1 }. i { $slot2 } giorni precedenti sono ancora nel tuo diario; quelle connessioni non sono ancora state costruite.

journal.window.entities.copy.conn.horizon.one = le connessioni dal tempo nello stesso luogo risalgono solo fino a { $slot1 }. un giorno precedente è ancora nel tuo diario; quelle connessioni non sono ancora state costruite.

journal.window.entities.copy.conn.index.action = controlla lo stato del sistema →

journal.window.entities.copy.conn.index.unavailable = le connessioni non sono disponibili in questo momento.

journal.window.entities.copy.conn.kind.chip.words.mentioned = menzioni

journal.window.entities.copy.conn.kind.words.attended.with = eventi

journal.window.entities.copy.conn.kind.words.co.present = in zona insieme

journal.window.entities.copy.conn.kind.words.created = creato

journal.window.entities.copy.conn.kind.words.family.of = famiglia

journal.window.entities.copy.conn.kind.words.knows = conosce

journal.window.entities.copy.conn.kind.words.mentioned = menzione

journal.window.entities.copy.conn.kind.words.messaged.with = messaggio

journal.window.entities.copy.conn.kind.words.other = in relazione

journal.window.entities.copy.conn.kind.words.party.of = parte in

journal.window.entities.copy.conn.kind.words.reports.to = fa capo a

journal.window.entities.copy.conn.kind.words.scheduled.with = in programma

journal.window.entities.copy.conn.kind.words.spoke.with = parlato

journal.window.entities.copy.conn.kind.words.uses = usa

journal.window.entities.copy.conn.kind.words.works.at = lavora presso

journal.window.entities.copy.conn.kind.words.works.with = lavora con

journal.window.entities.copy.conn.label.fallbacks.co.present = tempo nello stesso luogo

journal.window.entities.copy.conn.label.fallbacks.mentioned = se ne è parlato in conversazione

journal.window.entities.copy.conn.label.fallbacks.spoke.with = una conversazione

journal.window.entities.copy.conn.load.failed = non è stato possibile caricare le connessioni. riprova.

journal.window.entities.copy.conn.row.meta = { $slot1 } momenti · { $slot2 }

journal.window.entities.copy.conn.row.meta.attendance = { $slot1 } eventi insieme · { $slot2 }

journal.window.entities.copy.conn.row.meta.one = 1 momento · { $slot1 }

journal.window.entities.copy.conn.row.meta.one.attendance = 1 evento insieme · { $slot1 }

journal.window.entities.copy.conn.row.meta.scheduled = { $slot1 }

journal.window.entities.copy.conn.source.word.fallback = ciò che il tuo diario tiene

journal.window.entities.copy.conn.source.words.calendar = eventi del calendario

journal.window.entities.copy.conn.source.words.co.presence = momenti condivisi

journal.window.entities.copy.conn.source.words.document = documenti

journal.window.entities.copy.conn.source.words.event.legacy = eventi del calendario più vecchi

journal.window.entities.copy.conn.source.words.mention = conversazione

journal.window.entities.copy.conn.source.words.messaging = messaggi

journal.window.entities.copy.conn.source.words.observation = ciò che il tuo diario tiene

journal.window.entities.copy.conn.source.words.participation = registri delle attività

journal.window.entities.copy.conn.source.words.relation = ciò che il tuo diario tiene

journal.window.entities.copy.conn.source.words.speaker = conversazione

journal.window.entities.copy.conn.summary = { $slot1 } con prova diretta · { $slot2 } solo da eventi condivisi

journal.window.entities.copy.conn.summary.calendar = { $slot1 } solo dal tuo calendario

journal.window.entities.copy.conn.with.you.eyebrow = con te

journal.window.entities.copy.conn.with.you.failed = non è stato possibile controllare la tua cronologia con questa entità proprio ora.

journal.window.entities.copy.conn.with.you.summary = { $slot1 } momenti insieme · ultimo { $slot2 } ({ $slot3 })

journal.window.entities.copy.conn.with.you.summary.one = 1 momento insieme · ultimo { $slot1 } ({ $slot2 })

journal.window.entities.copy.conn.with.you.summary.scheduled = solo sul tuo calendario · ultimo { $slot1 }

journal.window.entities.copy.conn.with.you.toggle = i momenti dietro a questo ›

journal.window.entities.copy.detach.confirm = separa { $slot1 } da { $slot2 }? resta nel tuo diario e puoi allegarla di nuovo in qualsiasi momento.

journal.window.entities.copy.detach.done = separata da { $slot1 }. { $slot2 } è ancora nel tuo diario.

journal.window.entities.copy.detach.find.action = trovala nel tuo diario →

journal.window.entities.copy.detach.reattach.action = allegala di nuovo →

journal.window.entities.copy.detected.all.hidden = tutto ciò che è stato rilevato qui di recente è nascosto.

journal.window.entities.copy.detected.empty = qui non c'è ancora niente. man mano che aggiungi al tuo diario, persone, luoghi e cose compariranno qui.

journal.window.entities.copy.detected.hidden.badge = nascosto

journal.window.entities.copy.detected.hidden.count = { $slot1 } nomi nascosti in { $slot2 }

journal.window.entities.copy.detected.hidden.count.one = 1 nome nascosto in { $slot1 }

journal.window.entities.copy.detected.hidden.heading = nascosto in { $slot1 }

journal.window.entities.copy.detected.hidden.last = ultimo rilevamento { $slot1 }

journal.window.entities.copy.detected.hidden.lede = un nome nascosto resta nel tuo diario e resta nascosto anche se viene rilevato di nuovo. i suoi rilevamenti qui sono esclusi dalla ricerca, gli agenti connessi non possono raggiungerli, e il tuo diario non lo aggiunge a questa facetta.

journal.window.entities.copy.detected.hidden.show = mostra i nascosti

journal.window.entities.copy.detected.hidden.stop.show = smetti di mostrare i nascosti

journal.window.entities.copy.detected.hide.action = nascondi { $slot1 } in { $slot2 }

journal.window.entities.copy.detected.hide.done = { $slot1 } è nascosto in { $slot2 }.

journal.window.entities.copy.detected.hide.done.detail = resta nascosto anche se viene rilevato di nuovo.

journal.window.entities.copy.detected.hide.failed = non è stato possibile nascondere { $slot1 }. niente è cambiato.

journal.window.entities.copy.detected.hide.no.search = la ricerca e gli agenti connessi potrebbero ancora trovare i suoi rilevamenti qui.

journal.window.entities.copy.detected.hide.try.again = riprova

journal.window.entities.copy.detected.still.hidden = { $slot1 } corrisponde ancora a { $slot2 }, un'entità nascosta.

journal.window.entities.copy.detected.still.hidden.open = apri { $slot1 }

journal.window.entities.copy.detected.unhide = mostra di nuovo

journal.window.entities.copy.detected.unhide.failed = non è stato possibile mostrare di nuovo { $slot1 }. niente è cambiato.

journal.window.entities.copy.detected.unreadable = i rilevamenti non si possono mostrare qui. il tuo diario non è riuscito a leggere quali nomi hai nascosto in questa facetta.

journal.window.entities.copy.faces.busy = il tuo diario è occupato con i dati dei volti in questo momento, quindi niente è cambiato.

journal.window.entities.copy.faces.change = cambia la foto del profilo

journal.window.entities.copy.faces.changed = è cambiato da quando è stata caricata la pagina, quindi non è stato annullato.

journal.window.entities.copy.faces.chosen.alt = foto del profilo di { $slot1 }

journal.window.entities.copy.faces.confirmed.lede = hai scelto questa foto del profilo per { $slot1 }.

journal.window.entities.copy.faces.disabled = i suggerimenti per la foto del profilo sono disattivi, quindi niente è cambiato.

journal.window.entities.copy.faces.dismissed.lede = non hai scelto nessuna foto del profilo per { $slot1 }. se compaiono altri volti che potrebbero essere { $slot2 }, possono mostrarsi qui.

journal.window.entities.copy.faces.done.chosen = foto del profilo salvata per { $slot1 }.

journal.window.entities.copy.faces.done.dismissed = nessuna foto del profilo per { $slot1 }.

journal.window.entities.copy.faces.done.rejected = ok, quello non è { $slot1 }.

journal.window.entities.copy.faces.face.alt = un volto da una riunione il { $slot1 } che potrebbe essere { $slot2 }

journal.window.entities.copy.faces.facet.hint = scegli una foto del profilo per { $slot1 }

journal.window.entities.copy.faces.failed = non è stato salvato.

journal.window.entities.copy.faces.from.day = da una riunione il { $slot1 }

journal.window.entities.copy.faces.gone = quel volto non è più disponibile.

journal.window.entities.copy.faces.heading = foto del profilo

journal.window.entities.copy.faces.load.failed = non è stato possibile caricare questi volti.

journal.window.entities.copy.faces.merge.reason = i volti delle tue riunioni suggeriscono che potrebbero essere la stessa persona.

journal.window.entities.copy.faces.none = nessuno di questi

journal.window.entities.copy.faces.not.them = non { $slot1 }

journal.window.entities.copy.faces.possible.lede = questi volti delle tue riunioni potrebbero essere { $slot1 }. nessuno viene usato finché non ne scegli uno.

journal.window.entities.copy.faces.possible.title = scegli una foto del profilo per { $slot1 }

journal.window.entities.copy.faces.refresh = aggiorna

journal.window.entities.copy.faces.remove = non usare una foto del profilo

journal.window.entities.copy.faces.retry = riprova

journal.window.entities.copy.faces.see.others = scegline uno diverso

journal.window.entities.copy.faces.settings.link = attivali nelle impostazioni

journal.window.entities.copy.faces.show.again = annulla

journal.window.entities.copy.faces.source.anchored = accanto al loro nome

journal.window.entities.copy.faces.source.box = nel loro riquadro video

journal.window.entities.copy.faces.stale = è cambiato da quando è stata caricata la pagina, quindi non è stato salvato niente.

journal.window.entities.copy.faces.suggested.badge = suggerito

journal.window.entities.copy.faces.suggested.lede = il tuo diario ha scelto questo volto dalle tue registrazioni delle riunioni. è solo un suggerimento finché non dici di sì.

journal.window.entities.copy.faces.suggested.title = questo è { $slot1 }?

journal.window.entities.copy.faces.unconfirmed.alt = un volto da una riunione che potrebbe essere { $slot1 }, non confermato

journal.window.entities.copy.faces.unconfirmed.badge = non confermato

journal.window.entities.copy.faces.undo = annulla

journal.window.entities.copy.faces.undone = annullato.

journal.window.entities.copy.faces.use = usa questa foto del profilo

journal.window.entities.copy.faces.yes = sì, usa questa foto del profilo

journal.window.entities.copy.grid.clear.range = azzera l'intervallo

journal.window.entities.copy.grid.day.one = giorno

journal.window.entities.copy.grid.day.other = giorni

journal.window.entities.copy.grid.default.day = giorno più recente: { $slot1 }

journal.window.entities.copy.grid.lede = { $slot1 } { $slot2 } su { $slot3 } { $slot4 }

journal.window.entities.copy.grid.range = { $slot1 } – { $slot2 }

journal.window.entities.copy.grid.show.all = mostra tutti i { $slot1 } { $slot2 }

journal.window.entities.copy.grid.title = momenti nel tempo

journal.window.entities.copy.grid.unit.none = ancora niente

journal.window.entities.copy.grid.unit.one = momento

journal.window.entities.copy.grid.unit.other = momenti

journal.window.entities.copy.hide.pane.help = la sposta nei nascosti, dove puoi mostrarla di nuovo. le sue note, e i suoi rilevamenti nelle sue facette, escono dalla ricerca, e gli agenti connessi non possono raggiungerli.

journal.window.entities.copy.obs.disclosure = in mostra i { $slot1 } più recenti di { $slot2 } momenti

journal.window.entities.copy.obs.empty = ancora niente con { $slot1 }.

journal.window.entities.copy.obs.heading = momenti

journal.window.entities.copy.obs.load.failed = non è stato possibile caricare questi momenti. riprova →

journal.window.entities.copy.obs.source.link.title = vedi questo giorno

journal.window.entities.copy.scope.empty.action = mostra tutto il tuo diario

journal.window.entities.copy.scope.empty.body = le entità compaiono qui quando il tuo diario tiene persone, luoghi e progetti in questa facetta.

journal.window.entities.copy.scope.empty.title = ancora niente in { $slot1 }.

journal.window.entities.copy.scope.facet.missing = quella facetta non è più nel tuo diario, quindi questo è tutto il tuo diario.

journal.window.entities.copy.scope.showing = in mostra

journal.window.entities.copy.scope.whole.journal = tutto il tuo diario

journal.window.entities.copy.trust.action.failed = quella modifica dell'entità non è terminata.

journal.window.entities.copy.trust.heading = fiducia dell'identità

journal.window.entities.copy.trust.history.change = { $slot1 } → { $slot2 }

journal.window.entities.copy.trust.history.empty = non è ancora disponibile una cronologia dell'identità.

journal.window.entities.copy.trust.history.heading = cronologia delle versioni

journal.window.entities.copy.trust.history.suggested.by = suggerito da { $slot1 }

journal.window.entities.copy.trust.history.summary = { $slot1 } · { $slot2 } · { $slot3 }

journal.window.entities.copy.trust.identity.none = nessuna identità precedente

journal.window.entities.copy.trust.identity.summary = { $slot1 } · { $slot2 } · noto anche come: { $slot3 }

journal.window.entities.copy.trust.load.failed = non è stato possibile caricare i dettagli della fiducia dell'identità. riprova.

journal.window.entities.copy.trust.merge.action = anteprima dell'unione

journal.window.entities.copy.trust.merge.confirm = unisci { $slot1 } in { $slot2 }? un'unione non si può annullare.

journal.window.entities.copy.trust.merge.confirm.action = conferma l'unione

journal.window.entities.copy.trust.merge.done = unito.

journal.window.entities.copy.trust.merge.label = unisci in

journal.window.entities.copy.trust.preview.counts = campioni di voce aggiunti: { $slot1 } · campioni dopo l'unione: { $slot2 } · note sotto il nome che si unisce: { $slot3 } · etichette del parlante cambiate: { $slot4 } · registri delle attività cambiati: { $slot5 }

journal.window.entities.copy.trust.preview.summary = alias aggiunti: { $slot1 } · email aggiunte: { $slot2 }

journal.window.entities.copy.trust.restore.action = ripristina questa versione

journal.window.entities.copy.trust.restore.confirm = ripristina l'identità da questa versione? l'identità attuale resterà nella cronologia.

journal.window.entities.copy.trust.suggestion.summary = { $slot1 } → { $slot2 } · { $slot3 }

journal.window.entities.copy.trust.suggestions.empty = nessun suggerimento di unione aperto coinvolge questa entità.

journal.window.entities.copy.trust.suggestions.heading = suggerimenti di unione aperti

journal.window.entities.copy.voice.changed = quel campione è cambiato. aggiorna l'elenco prima di rimuoverlo.

journal.window.entities.copy.voice.empty = nessun campione di voce salvato per questa persona.

journal.window.entities.copy.voice.error = non è stato possibile leggere o cambiare quei campioni di voce. riprova.

journal.window.entities.copy.voice.heading = campioni di voce

journal.window.entities.copy.voice.loading = caricamento dei campioni di voce…

journal.window.entities.copy.voice.more = mostra altro

journal.window.entities.copy.voice.open = apri la registrazione

journal.window.entities.copy.voice.remove = rimuovi

journal.window.entities.copy.voice.retry = riprova

journal.window.entities.copy.voice.sentence = frase { $slot1 }

journal.window.entities.delete.all.items.gone = { $removed_quantity ->
    [one] non è più in elenco qui.
    [many] non sono più in elenco qui.
   *[other] non sono più in elenco qui.
    }

journal.window.entities.delete.couldn.t.erase.that = non è stato possibile cancellare quell'entità

journal.window.entities.delete.couldn.t.recover.that = non è stato possibile recuperare quell'entità

journal.window.entities.delete.detail.action.help = { $action_start }elimina{ $action_end } { $description_start }la sposta nel cestino, dove puoi recuperarla finché non viene cancellata.{ $description_end }

journal.window.entities.delete.erased.from.your.journal = { $total_quantity ->
    [one] cancellata { $total } entità dal tuo diario
    [many] cancellate { $total } di entità dal tuo diario
   *[other] cancellate { $total } entità dal tuo diario
    }

journal.window.entities.delete.erased.of = { $processed_quantity ->
    [one] cancellato { $processed } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
    [many] cancellato { $processed } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
   *[other] cancellato { $processed } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
    }

journal.window.entities.delete.erases.unless.you.recover.it = viene cancellata { $erase_day } se non la recuperi.

journal.window.entities.delete.facet.many.its.place.and.notes.in = { $facets_quantity ->
    [one] il suo luogo e le sue note in { $facets } facetta
    [many] il suo luogo e le sue note in { $facets } di facette
   *[other] il suo luogo e le sue note in { $facets } facette
    }

journal.window.entities.delete.gone.more.many.more.are.no.longer.listed = { $gone_quantity ->
    [one] { $gone } altro non è più in elenco.
    [many] { $gone } di altri non sono più in elenco.
   *[other] { $gone } altri non sono più in elenco.
    }

journal.window.entities.delete.items.find.after.failed.erase = { $remaining_quantity ->
    [one] cercala sotto o nel cestino.
    [many] cercale sotto o nel cestino.
   *[other] cercale sotto o nel cestino.
    }

journal.window.entities.delete.many.delete.entities = { $total_quantity ->
    [one] elimina { $total } entità?
    [many] elimina { $total } di entità?
   *[other] elimina { $total } entità?
    }

journal.window.entities.delete.many.erase.entities.now = { $total_quantity ->
    [one] cancella { $total } entità ora?
    [many] cancella { $total } di entità ora?
   *[other] cancella { $total } entità ora?
    }

journal.window.entities.delete.marked.many.couldn.t.be.moved.and = { $remaining_quantity ->
    [one] { $remaining } non si è potuta spostare ed è segnata sotto.
    [many] { $remaining } non si sono potute spostare e sono segnate sotto.
   *[other] { $remaining } non si sono potute spostare e sono segnate sotto.
    }

journal.window.entities.delete.more.and.more = { $more_quantity ->
    [one] e { $more } in più
    [many] e { $more } in più
   *[other] e { $more } in più
    }

journal.window.entities.delete.moved.of.to.the = { $processed_quantity ->
    [one] spostato { $processed } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    } nel cestino
    [many] spostato { $processed } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    } nel cestino
   *[other] spostato { $processed } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    } nel cestino
    }

journal.window.entities.delete.moved.to.the.trash = { $total_quantity ->
    [one] spostata { $total } entità nel cestino
    [many] spostate { $total } di entità nel cestino
   *[other] spostate { $total } entità nel cestino
    }

journal.window.entities.delete.none.still.many.are.still.in.your.journal = { $remaining_quantity ->
    [one] { $remaining } è ancora nel tuo diario, segnata sotto.
    [many] { $remaining } sono ancora nel tuo diario, segnate sotto.
   *[other] { $remaining } sono ancora nel tuo diario, segnate sotto.
    }

journal.window.entities.delete.noun.erased.from.your.journal = entità cancellata dal tuo diario

journal.window.entities.delete.other.items.gone = { $other_quantity ->
    [one] l'altra non è più in elenco qui.
    [many] le altre non sono più in elenco qui.
   *[other] le altre non sono più in elenco qui.
    }

journal.window.entities.delete.partial.items.not.erased = { $remaining_quantity ->
    [one] l'altra non è stata cancellata. cercala sotto o nel cestino.
    [many] le altre non sono state cancellate. cercale sotto o nel cestino.
   *[other] le altre non sono state cancellate. cercale sotto o nel cestino.
    }

journal.window.entities.delete.partial.items.not.recovered = { $remaining_quantity ->
    [one] l'altra è ancora nel cestino. puoi riprovare da lì.
    [many] le altre sono ancora nel cestino. puoi riprovare da lì.
   *[other] le altre sono ancora nel cestino. puoi riprovare da lì.
    }

journal.window.entities.delete.pick.label.select.name = seleziona { $entity_name }

journal.window.entities.delete.recovered.count = { $total_quantity ->
    [one] { $total } entità recuperata
    [many] { $total } di entità recuperate
   *[other] { $total } entità recuperate
    }

journal.window.entities.delete.recovered.of = { $processed_quantity ->
    [one] recuperato { $processed } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
    [many] recuperato { $processed } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
   *[other] recuperato { $processed } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
    }

journal.window.entities.delete.recovered.one = entità recuperata

journal.window.entities.delete.selected.selected = { $selected_quantity ->
    [one] { $selected } selezionata
    [many] { $selected } selezionate
   *[other] { $selected } selezionate
    }

journal.window.entities.delete.still.many.couldn.t.be.moved.and = { $remaining_quantity ->
    [one] { $remaining } non si è potuta spostare ed è ancora nel tuo diario, segnata sotto.
    [many] { $remaining } non si sono potute spostare e sono ancora nel tuo diario, segnate sotto.
   *[other] { $remaining } non si sono potute spostare e sono ancora nel tuo diario, segnate sotto.
    }

journal.window.entities.delete.they.erase.unless.you.recover = vengono cancellate { $erase_day } se non le recuperi.

journal.window.entities.detail.aliases.other.names = Noto anche come: { $aliases }

journal.window.entities.detail.facets.count = { $facets_quantity ->
    [one] 1 facetta
    [many] { $facets } di facette
   *[other] { $facets } facette
    }

journal.window.entities.detail.id.description = questo nome sembra un ID.

journal.window.entities.detail.last.mentioned.date = Ultima menzione: { $day }

journal.window.entities.detail.last.mentioned.full.day = Ultima menzione: { $day }

journal.window.entities.detected.hidden.heading = nascosto in { $facet_name }

journal.window.entities.detected.hidden.in.facet = { $hidden_quantity ->
        [one] 1 nome nascosto in { $facet_name }
        [many] { $hidden } di nomi nascosti in { $facet_name }
       *[other] { $hidden } nomi nascosti in { $facet_name }
    }

journal.window.entities.detected.hidden.last.detected = ultimo rilevamento { $day }

journal.window.entities.detected.hidden.reappeared = { $entity_name } corrisponde ancora a { $matched_name }, un'entità nascosta.

journal.window.entities.detected.hidden.reappeared.open = apri { $matched_name }

journal.window.entities.detected.hide.action = nascondi { $entity_name } in { $facet_name }

journal.window.entities.detected.hide.done = { $entity_name } è nascosto in { $facet_name }.

journal.window.entities.detected.hide.failed = non è stato possibile nascondere { $entity_name }. niente è cambiato.

journal.window.entities.detected.page.range = { $total_quantity ->
        [one] in mostra { $first }–{ $last } di { $total } entità rilevata
        [many] in mostra { $first }–{ $last } di { $total } di entità rilevate
       *[other] in mostra { $first }–{ $last } di { $total } entità rilevate
    }

journal.window.entities.detected.unhide.failed = non è stato possibile mostrare di nuovo { $entity_name }. niente è cambiato.

journal.window.entities.entity.delete.copy.again.if.your.journal.finds.it.again = se il tuo diario la trova di nuovo più tardi, viene aggiunta come nuova entità.

journal.window.entities.entity.delete.copy.again.many.any.of.them.your.journal.finds = qualunque di esse che il tuo diario trova di nuovo più tardi viene aggiunta come nuova entità.

journal.window.entities.entity.delete.copy.back.back = indietro

journal.window.entities.entity.delete.copy.busy.something.else.is.changing.it.right = qualcos'altro la sta cambiando proprio ora. riprova tra un minuto.

journal.window.entities.entity.delete.copy.busy.something.else.is.changing.this.right = qualcos'altro sta cambiando questa proprio ora, quindi non è stata spostata. riprova tra un minuto.

journal.window.entities.entity.delete.copy.busy.something.else.is.changing.this.right.variant2 = qualcos'altro sta cambiando questa proprio ora, quindi non è stata cancellata. riprova tra un minuto.

journal.window.entities.entity.delete.copy.button.delete = elimina

journal.window.entities.entity.delete.copy.cancel.cancel = annulla

journal.window.entities.entity.delete.copy.delete.delete = elimina

journal.window.entities.entity.delete.copy.details.its.details.and.version.history = i suoi dettagli e la sua cronologia delle versioni

journal.window.entities.entity.delete.copy.done.done = fatto

journal.window.entities.entity.delete.copy.erase.now.erase.now = cancella ora

journal.window.entities.entity.delete.copy.facet.one.its.place.and.notes.in.facet = il suo luogo e le sue note in 1 facetta

journal.window.entities.entity.delete.copy.facet.unknown.its.place.and.notes.in.each = il suo luogo e le sue note in ogni facetta in cui è

journal.window.entities.entity.delete.copy.gone.it.s.no.longer.in.your = non è più nel tuo diario.

journal.window.entities.entity.delete.copy.last.facet.your.journal.needs.at.least.one = al tuo diario serve almeno una facetta non silenziata.

journal.window.entities.entity.delete.copy.lead.this.erases.the.entity.s.own = questo cancella ciò che è proprio dell'entità:

journal.window.entities.entity.delete.copy.lead.this.moves.the.entity.s.own = questo sposta ciò che è proprio dell'entità:

journal.window.entities.entity.delete.copy.move.to.trash.move.to.trash = sposta nel cestino

journal.window.entities.entity.delete.copy.open.trash.open.trash = apri il cestino

journal.window.entities.entity.delete.copy.other.this.wasn.t.erased.try.again = questa non è stata cancellata. riprova.

journal.window.entities.entity.delete.copy.other.this.wasn.t.moved.try.again = questa non è stata spostata. riprova.

journal.window.entities.entity.delete.copy.own.many.this.erases.each.one.s.own = questo cancella di ognuna i propri dettagli, la propria cronologia delle versioni, il proprio luogo e le proprie note in ogni facetta, e ciò che il tuo diario tiene sulla sua voce, se ce n'è.

journal.window.entities.entity.delete.copy.own.many.this.moves.each.one.s.own = questo sposta di ognuna i propri dettagli, la propria cronologia delle versioni, il proprio luogo e le proprie note in ogni facetta, e ciò che il tuo diario tiene sulla sua voce, se ce n'è.

journal.window.entities.entity.delete.copy.principal.this.is.you.so.it.can = sei tu, quindi non si può eliminare.

journal.window.entities.entity.delete.copy.reach.it.leaves.your.journal.now.and = esce ora dal tuo diario e non si può recuperare.

journal.window.entities.entity.delete.copy.reach.it.leaves.your.journal.now.and.variant2 = esce dal tuo diario ora e aspetta nel cestino, poi viene cancellata. puoi recuperarla fino ad allora.

journal.window.entities.entity.delete.copy.reach.many.they.leave.your.journal.now.and = escono ora dal tuo diario e non si possono recuperare.

journal.window.entities.entity.delete.copy.reach.many.they.leave.your.journal.now.and.variant2 = escono dal tuo diario ora e aspettano nel cestino, poi vengono cancellate. puoi recuperare ognuna fino ad allora.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its = non è stato possibile aggiornare il registro delle entità del tuo diario. niente è stato spostato.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its.variant2 = non è stato possibile aggiornare il registro delle entità del tuo diario. niente è stato cancellato.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its.variant3 = non è stato possibile aggiornare il registro delle entità del tuo diario, quindi questa non è stata spostata.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its.variant4 = non è stato possibile aggiornare il registro delle entità del tuo diario, quindi questa non è stata cancellata.

journal.window.entities.entity.delete.copy.recover.recover = ripristina

journal.window.entities.entity.delete.copy.select.all.page.select.all.on.this.page = seleziona tutto in questa pagina

journal.window.entities.entity.delete.copy.select.all.select.all = seleziona tutto

journal.window.entities.entity.delete.copy.select.select = seleziona

journal.window.entities.entity.delete.copy.stays.many.transcripts.summaries.and.activities.elsewhere.in = le trascrizioni, i riepiloghi e le attività altrove nel tuo diario non vengono cancellati e potrebbero ancora menzionarle.

journal.window.entities.entity.delete.copy.stays.many.transcripts.summaries.and.activities.elsewhere.in.variant2 = le trascrizioni, i riepiloghi e le attività altrove nel tuo diario restano dove sono e potrebbero ancora menzionarle.

journal.window.entities.entity.delete.copy.stays.transcripts.summaries.and.activities.elsewhere.in = le trascrizioni, i riepiloghi e le attività altrove nel tuo diario non vengono cancellati e potrebbero ancora menzionarla.

journal.window.entities.entity.delete.copy.stays.transcripts.summaries.and.activities.elsewhere.in.variant2 = le trascrizioni, i riepiloghi e le attività altrove nel tuo diario restano dove sono e potrebbero ancora menzionarla.

journal.window.entities.entity.delete.copy.voice.unknown.what.your.journal.keeps.about.its = ciò che il tuo diario tiene sulla sua voce, se ce n'è

journal.window.entities.entity.delete.copy.voice.what.your.journal.keeps.about.its = ciò che il tuo diario tiene sulla sua voce

journal.window.entities.examples.first.and.second = ad es. { $first_name } e { $second_name }

journal.window.entities.examples.names = ad es. { $names }

journal.window.entities.faces.chosen.alt = foto del profilo di { $entity_name }

journal.window.entities.faces.confirmed.lede = hai scelto questa foto del profilo per { $entity_name }.

journal.window.entities.faces.dismissed.lede = non hai scelto nessuna foto del profilo per { $entity_name }. se compaiono altri volti che potrebbero essere { $entity_name }, potrebbero mostrarsi qui.

journal.window.entities.faces.done.chosen = foto del profilo salvata per { $entity_name }.

journal.window.entities.faces.done.dismissed = nessuna foto del profilo per { $entity_name }.

journal.window.entities.faces.done.rejected = ok, quello non è { $entity_name }.

journal.window.entities.faces.face.alt = un volto da una riunione del { $day } che potrebbe essere { $entity_name }

journal.window.entities.faces.facet.hint = scegli una foto del profilo per { $entity_name }

journal.window.entities.faces.from.day = da una riunione del { $day }

journal.window.entities.faces.not.them = non { $entity_name }

journal.window.entities.faces.possible.lede = questi volti dalle tue riunioni potrebbero essere { $entity_name }. nessuno viene usato finché non ne scegli uno.

journal.window.entities.faces.possible.title = scegli una foto del profilo per { $entity_name }

journal.window.entities.faces.suggested.title = questo è { $entity_name }?

journal.window.entities.faces.unconfirmed.alt = un volto da una riunione che potrebbe essere { $entity_name }, non confermato

journal.window.entities.facet.detach.confirm = separa { $entity_name } da { $facet_name }? resta nel tuo diario e puoi allegarla di nuovo in qualsiasi momento.

journal.window.entities.facet.detach.done = separata da { $facet_name }. { $entity_name } è ancora nel tuo diario.

journal.window.entities.facet.empty.heading = non c'è ancora niente in { $facet_name }.

journal.window.entities.facet.membership.list = in { $leading_facets } e { $final_facet }

journal.window.entities.facet.membership.single = in { $facet_name }

journal.window.entities.field.label.alternative.names.akas = nomi alternativi (alias)

journal.window.entities.field.label.description = descrizione

journal.window.entities.field.label.name = nome

journal.window.entities.field.label.type = tipo

journal.window.entities.field.placeholder.alias1.alias2.alias3 = alias1, alias2, alias3

journal.window.entities.field.placeholder.e.g.lead.engineer.on.the = ad es., ingegnere capo sul progetto, contatto principale per le vendite…

journal.window.entities.field.placeholder.enter.description = inserisci la descrizione…

journal.window.entities.field.placeholder.search.entities = cerca entità…

journal.window.entities.field.placeholder.type.a.name.to.find.it = scrivi un nome per trovarla

journal.window.entities.folder.all = tutte

journal.window.entities.folder.companies = aziende

journal.window.entities.folder.duplicates = possibili duplicati

journal.window.entities.folder.hidden = nascosto

journal.window.entities.folder.other = altro

journal.window.entities.folder.people = persone

journal.window.entities.folder.projects = progetti

journal.window.entities.folder.tools = strumenti

journal.window.entities.heading.couldn.t.load.entities = Non è stato possibile caricare le entità

journal.window.entities.heading.that.search.couldn.t.be.run = non è stato possibile eseguire quella ricerca

journal.window.entities.heading.this.entity.couldn.t.be.loaded = non è stato possibile caricare questa entità

journal.window.entities.hide.already.absent.count = { $existing_quantity ->
        [one] 1 era già nascosta o non c'era più.
        [many] { $existing } erano già nascoste o non c'erano più.
       *[other] { $existing } erano già nascoste o non c'erano più.
    }

journal.window.entities.hide.detail.action.help = { $action_start }nascondi{ $action_end } { $description_start }la sposta in nascosto, dove puoi mostrarla di nuovo. le sue note, e i suoi rilevamenti nelle sue facette, escono dalla ricerca, e gli agenti connessi non possono raggiungerli.{ $description_end }

journal.window.entities.hide.done.count = { $hidden_quantity ->
        [one] nascosta 1 entità.
        [many] nascoste { $hidden } di entità.
       *[other] nascoste { $hidden } entità.
    }

journal.window.entities.hide.partial = { $remaining_quantity ->
        [one] nascoste { $hidden_quantity ->
            [one] { $hidden }
            [many] { $hidden }
           *[other] { $hidden }
        } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }. il resto non era nascosto.
        [many] nascoste { $hidden_quantity ->
            [one] { $hidden }
            [many] { $hidden }
           *[other] { $hidden }
        } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }. le altre non erano nascoste.
       *[other] nascoste { $hidden_quantity ->
            [one] { $hidden }
            [many] { $hidden }
           *[other] { $hidden }
        } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }. le altre non erano nascoste.
    }

journal.window.entities.hide.paused.records = { $records_quantity ->
        [one] il nascondimento è in pausa: un registro nel tuo diario non si può leggere ({ $paths }).
        [many] il nascondimento è in pausa: { $records } di registri nel tuo diario non si possono leggere ({ $paths }).
       *[other] il nascondimento è in pausa: { $records } registri nel tuo diario non si possono leggere ({ $paths }).
    }

journal.window.entities.hide.progress = { $done_quantity ->
        [one] nascondimento in corso… { $done } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
        [many] nascondimento in corso… { $done } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
       *[other] nascondimento in corso… { $done } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
    }

journal.window.entities.hide.record.error = non è stato possibile nasconderle: un registro nel tuo diario non si può leggere ({ $record_detail }).

journal.window.entities.hide.selection.count = { $selected_quantity ->
        [one] nascondi { $selected }
        [many] nascondi { $selected }
       *[other] nascondi { $selected }
    }

journal.window.entities.hide.selection.progress = { $done_quantity ->
        [one] nascondimento in corso… { $done } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
        [many] nascondimento in corso… { $done } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
       *[other] nascondimento in corso… { $done } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
    }

journal.window.entities.hide.undo.done = { $restored_quantity ->
        [one] rimessa 1 entità.
        [many] rimesse { $restored } di entità.
       *[other] rimesse { $restored } entità.
    }

journal.window.entities.hide.undo.help = { $hidden_quantity ->
        [one] l'annullamento la rimette, facette incluse. è anche sotto nascosto ogni volta che la vuoi.
        [many] l'annullamento le rimette, facette incluse. sono anche sotto nascosto ogni volta che le vuoi.
       *[other] l'annullamento le rimette, facette incluse. sono anche sotto nascosto ogni volta che le vuoi.
    }

journal.window.entities.hide.undo.partial = { $failed_quantity ->
        [one] rimesse { $restored_quantity ->
            [one] { $restored }
            [many] { $restored }
           *[other] { $restored }
        } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }. 1 non si è potuta rimettere; è ancora sotto nascosto.
        [many] rimesse { $restored_quantity ->
            [one] { $restored }
            [many] { $restored }
           *[other] { $restored }
        } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }. { $failed } non si sono potute rimettere; sono ancora sotto nascosto.
       *[other] rimesse { $restored_quantity ->
            [one] { $restored }
            [many] { $restored }
           *[other] { $restored }
        } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }. { $failed } non si sono potute rimettere; sono ancora sotto nascosto.
    }

journal.window.entities.hide.undo.progress = { $restored_quantity ->
        [one] rimessa in corso… { $restored } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
        [many] rimessa in corso… { $restored } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
       *[other] rimessa in corso… { $restored } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
    }

journal.window.entities.hide.undo.records.missing.one.entity = { $records_quantity ->
        [one] potrebbe essere tornata senza tutte le sue facette: un registro nel tuo diario non si può leggere ({ $paths }).
        [many] potrebbe essere tornata senza tutte le sue facette: { $records } di registri nel tuo diario non si possono leggere ({ $paths }).
       *[other] potrebbe essere tornata senza tutte le sue facette: { $records } registri nel tuo diario non si possono leggere ({ $paths }).
    }

journal.window.entities.hide.undo.records.missing.several.entities = { $records_quantity ->
        [one] alcune potrebbero essere tornate senza tutte le loro facette: un registro nel tuo diario non si può leggere ({ $paths }).
        [many] alcune potrebbero essere tornate senza tutte le loro facette: { $records } di registri nel tuo diario non si possono leggere ({ $paths }).
       *[other] alcune potrebbero essere tornate senza tutte le loro facette: { $records } registri nel tuo diario non si possono leggere ({ $paths }).
    }

journal.window.entities.hide.undo.stopped = { $restored_quantity ->
        [one] l'annullamento si è fermato dopo aver rimesso { $restored } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }. le altre sono ancora sotto nascosto.
        [many] l'annullamento si è fermato dopo aver rimesso { $restored } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }. le altre sono ancora sotto nascosto.
       *[other] l'annullamento si è fermato dopo aver rimesso { $restored } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }. le altre sono ancora sotto nascosto.
    }

journal.window.entities.link.label.back.to.entities = ← indietro alle entità

journal.window.entities.link.label.edit = modifica

journal.window.entities.link.label.in.all.facets = in tutte le facette →

journal.window.entities.list.heading.facet = entità in { $facet_name }

journal.window.entities.list.last.mentioned.short.day = ultima menzione { $day }

journal.window.entities.list.page.range = { $first }–{ $last } di { $total }

journal.window.entities.list.results.count = { $matched_quantity ->
        [one] { $matched } di { $total_quantity ->
            [one] { $total } entità
            [many] { $total } di entità
           *[other] { $total } entità
        }
        [many] { $matched } di { $total_quantity ->
            [one] { $total } entità
            [many] { $total } di entità
           *[other] { $total } entità
        }
       *[other] { $matched } di { $total_quantity ->
            [one] { $total } entità
            [many] { $total } di entità
           *[other] { $total } entità
        }
    }

journal.window.entities.list.select.all.count = { $total_quantity ->
        [one] seleziona tutto { $total }
        [many] seleziona tutto { $total }
       *[other] seleziona tutto { $total }
    }

journal.window.entities.list.selected.count = { $selected_quantity ->
        [one] { $selected } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        } spuntate
        [many] { $selected } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        } spuntate
       *[other] { $selected } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        } spuntate
    }

journal.window.entities.list.sort.name = per nome

journal.window.entities.list.sort.recent = recenti

journal.window.entities.list.sort.type = per tipo

journal.window.entities.merge.confirm.selection = { $selected_quantity ->
        [one] unisci { $selected }
        [many] unisci { $selected }
       *[other] unisci { $selected }
    }

journal.window.entities.merge.evidence.meeting.photos = le foto delle riunioni suggeriscono che possa trattarsi della stessa persona.

journal.window.entities.merge.keep.done = { $total_quantity ->
        [one] tenuta separata 1 coppia.
        [many] tenute separate { $total } di coppie.
       *[other] tenute separate { $total } coppie.
    }

journal.window.entities.merge.keep.partial = { $failed_quantity ->
        [one] tenute separate { $processed_quantity ->
            [one] { $processed }
            [many] { $processed }
           *[other] { $processed }
        } di { $total_quantity ->
            [one] { $total } coppia
            [many] { $total } di coppie
           *[other] { $total } coppie
        }. 1 è ancora in elenco.
        [many] tenute separate { $processed_quantity ->
            [one] { $processed }
            [many] { $processed }
           *[other] { $processed }
        } di { $total_quantity ->
            [one] { $total } coppia
            [many] { $total } di coppie
           *[other] { $total } coppie
        }. { $failed } sono ancora in elenco.
       *[other] tenute separate { $processed_quantity ->
            [one] { $processed }
            [many] { $processed }
           *[other] { $processed }
        } di { $total_quantity ->
            [one] { $total } coppia
            [many] { $total } di coppie
           *[other] { $total } coppie
        }. { $failed } sono ancora in elenco.
    }

journal.window.entities.merge.keep.selection = { $selected_quantity ->
        [one] tieni { $selected } separati
        [many] tieni { $selected } separati
       *[other] tieni { $selected } separati
    }

journal.window.entities.merge.merge.done = { $total_quantity ->
        [one] unita 1 coppia.
        [many] unite { $total } di coppie.
       *[other] unite { $total } coppie.
    }

journal.window.entities.merge.merge.partial = { $failed_quantity ->
        [one] unite { $processed_quantity ->
            [one] { $processed }
            [many] { $processed }
           *[other] { $processed }
        } di { $total_quantity ->
            [one] { $total } coppia
            [many] { $total } di coppie
           *[other] { $total } coppie
        }. 1 non si è potuta unire.
        [many] unite { $processed_quantity ->
            [one] { $processed }
            [many] { $processed }
           *[other] { $processed }
        } di { $total_quantity ->
            [one] { $total } coppia
            [many] { $total } di coppie
           *[other] { $total } coppie
        }. { $failed } non si sono potute unire.
       *[other] unite { $processed_quantity ->
            [one] { $processed }
            [many] { $processed }
           *[other] { $processed }
        } di { $total_quantity ->
            [one] { $total } coppia
            [many] { $total } di coppie
           *[other] { $total } coppie
        }. { $failed } non si sono potute unire.
    }

journal.window.entities.merge.pair.confirm.aria = { $pairs_quantity ->
        [one] unisci 1 coppia?
        [many] unisci { $pairs } di coppie?
       *[other] unisci { $pairs } coppie?
    }

journal.window.entities.merge.pair.confirm.heading = { $pairs_quantity ->
        [one] unisci 1 coppia?
        [many] unisci { $pairs } di coppie?
       *[other] unisci { $pairs } coppie?
    }

journal.window.entities.merge.preview.direction = { $source_name } si unisce a { $target_name }

journal.window.entities.merge.progress = { $done_quantity ->
        [one] unione in corso… { $done } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
        [many] unione in corso… { $done } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
       *[other] unione in corso… { $done } di { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
    }

journal.window.entities.merge.progress.initial = { $total_quantity ->
        [one] unione in corso… 0 di { $total }
        [many] unione in corso… 0 di { $total }
       *[other] unione in corso… 0 di { $total }
    }

journal.window.entities.merge.unmerged.pair.count = { $pairs_quantity ->
        [one] 1 coppia in più esce dall'elenco senza unirsi: include un'entità già unita più sopra.
        [many] { $pairs } di coppie in più escono dall'elenco senza unirsi: ognuna include un'entità già unita più sopra.
       *[other] { $pairs } coppie in più escono dall'elenco senza unirsi: ognuna include un'entità già unita più sopra.
    }

journal.window.entities.message.it.may.already.be.back.in = potrebbe essere già di nuovo nel tuo diario. ricarica per controllare.

journal.window.entities.message.it.may.already.be.erased.reload = potrebbe essere già cancellato. ricarica per controllare.

journal.window.entities.message.it.may.already.be.in.the = potrebbe essere già nel cestino. ricarica per controllare.

journal.window.entities.message.it.s.back.in.your.journal = è di nuovo nel tuo diario.

journal.window.entities.message.it.s.in.the.trash.you = è nel cestino. puoi riprovare da lì.

journal.window.entities.message.it.s.still.in.the.trash = è ancora nel cestino. puoi riprovare da lì.

journal.window.entities.message.it.s.still.in.your.journal = è ancora nel tuo diario.

journal.window.entities.message.they.may.already.be.back.in = potrebbero essere già di nuovo nel tuo diario. ricarica per controllare.

journal.window.entities.message.they.may.already.be.erased.reload = potrebbero essere già state cancellate. ricarica per controllare.

journal.window.entities.message.they.may.already.be.in.the = potrebbero essere già nel cestino. ricarica per controllare.

journal.window.entities.message.they.re.back.in.your.journal = sono di nuovo nel tuo diario.

journal.window.entities.message.they.re.still.in.the.trash = sono ancora nel cestino. puoi riprovare da lì.

journal.window.entities.notes.count = { $notes_quantity ->
        [one] 1 nota
        [many] { $notes } di note
       *[other] { $notes } note
    }

journal.window.entities.notes.none = nessuna nota

journal.window.entities.observation.coverage = { $observations_quantity ->
        [one] { $observations } osservazione nell'arco di { $days_quantity ->
            [one] { $days } giorno
            [many] { $days } di giorni
           *[other] { $days } giorni
        }
        [many] { $observations } di osservazioni nell'arco di { $days_quantity ->
            [one] { $days } giorno
            [many] { $days } di giorni
           *[other] { $days } giorni
        }
       *[other] { $observations } osservazioni nell'arco di { $days_quantity ->
            [one] { $days } giorno
            [many] { $days } di giorni
           *[other] { $days } giorni
        }
    }

journal.window.entities.observation.disclosure = { $visible_quantity ->
        [one] in mostra i { $visible } più recenti di { $total_quantity ->
            [one] { $total } momento
            [many] { $total } di momenti
           *[other] { $total } momenti
        }
        [many] in mostra i { $visible } più recenti di { $total_quantity ->
            [one] { $total } momento
            [many] { $total } di momenti
           *[other] { $total } momenti
        }
       *[other] in mostra i { $visible } più recenti di { $total_quantity ->
            [one] { $total } momento
            [many] { $total } di momenti
           *[other] { $total } momenti
        }
    }

journal.window.entities.observation.grid.count = { $quantity ->
        [one] 1 momento
        [many] { $count } di momenti
       *[other] { $count } momenti
    }

journal.window.entities.observation.latest.day = giorno più recente: { $day }

journal.window.entities.observation.none.with.entity = non c'è ancora niente con { $entity_name }.

journal.window.entities.observation.range = { $start_day } – { $end_day }

journal.window.entities.observation.show.all = { $observations_quantity ->
        [one] mostra { $observations } osservazione
        [many] mostra tutte le { $observations } di osservazioni
       *[other] mostra tutte le { $observations } osservazioni
    }

journal.window.entities.response.alias.conflict = L'alias '{ $alias }' è in conflitto con l'entità '{ $conflicting_name }'.

journal.window.entities.response.alias.conflict.rename = L'alias '{ $alias }' è in conflitto con l'entità '{ $conflicting_name }'

journal.window.entities.response.ambiguity.dismiss.status = impossibile ignorare l'ambiguità con stato { $ambiguity_status }

journal.window.entities.response.ambiguity.file.corrupt = il file di ambiguità { $file_path } contiene una riga corrotta

journal.window.entities.response.candidate.facet.already.exists = La facetta '{ $facet }' esiste già

journal.window.entities.response.candidate.facet.candidate.accept.failed = accettazione della candidata non riuscita

journal.window.entities.response.candidate.facet.candidate.dismiss.failed = non è stato possibile ignorare la candidata

journal.window.entities.response.candidate.facet.candidate.read.failed = lettura della candidata non riuscita

journal.window.entities.response.candidate.facet.facet.creation.failed = creazione della facetta non riuscita

journal.window.entities.response.candidate.facet.facet.lookup.failed = ricerca della facetta non riuscita

journal.window.entities.response.candidate.facet.slug.invalid = Nome di facetta non valido '{ $facet }': deve essere minuscolo, iniziare con una lettera e contenere solo lettere, cifre, trattini o trattini bassi

journal.window.entities.response.candidate.not.found = candidata non trovata

journal.window.entities.response.candidate.record.missing = l'unione della candidata non ha un registro attivo corrispondente

journal.window.entities.response.candidate.status.accept = impossibile accettare la candidata con stato{ $slot1 }{ $status }

journal.window.entities.response.candidate.status.dismiss = impossibile ignorare la candidata con stato{ $slot1 }{ $status }

journal.window.entities.response.candidate.status.preview = impossibile vedere l'anteprima della candidata con stato{ $slot1 }{ $status }

journal.window.entities.response.delete.refusal.ambiguities.unreadable = non è stato possibile leggere i registri di ambiguità del tuo diario

journal.window.entities.response.delete.refusal.bad.request = non è stato possibile leggere quell'eliminazione

journal.window.entities.response.delete.refusal.busy = il tuo diario è occupato; riprova tra un momento

journal.window.entities.response.delete.refusal.failed = non è stato possibile spostarla nel cestino

journal.window.entities.response.delete.refusal.gone = quell'entità non è nel tuo diario

journal.window.entities.response.delete.refusal.link.unreadable = non è stato possibile leggere un collegamento di facetta

journal.window.entities.response.delete.refusal.principal = non puoi eliminarti

journal.window.entities.response.delete.refusal.record.damaged = non è stato possibile aggiornare il registro delle entità eliminate del tuo diario

journal.window.entities.response.detail.access.denied = accesso negato

journal.window.entities.response.detail.after.id.and.offset.cannot.be.used.together = { $slot1 } e offset non si possono usare insieme

journal.window.entities.response.detail.after.id.must.be.a.non.negative.integer = { $slot1 } deve essere un intero non negativo

journal.window.entities.response.detail.aka.is.required = l'alias è obbligatorio

journal.window.entities.response.detail.ambiguity.dismiss.failed = non è stato possibile ignorare l'ambiguità

journal.window.entities.response.detail.ambiguity.id.is.required = { $slot1 } è obbligatorio

journal.window.entities.response.detail.ambiguity.not.found = ambiguità non trovata

journal.window.entities.response.detail.ambiguity.read.failed = lettura dell'ambiguità non riuscita

journal.window.entities.response.detail.ambiguity.resolve.failed = risoluzione dell'ambiguità non riuscita

journal.window.entities.response.detail.batch.block.failed = blocco del lotto non riuscito

journal.window.entities.response.detail.batch.unblock.failed = sblocco del lotto non riuscito

journal.window.entities.response.detail.body.is.not.valid.json = il corpo non è { $slot1 } valido

journal.window.entities.response.detail.body.must.be.a.json.object = il corpo deve essere un oggetto { $slot1 }

journal.window.entities.response.detail.candidate.read.failed = lettura della candidata non riuscita

journal.window.entities.response.detail.content.is.required = il contenuto è obbligatorio

journal.window.entities.response.detail.couldn.t.hide.entity = non è stato possibile nascondere l'entità

journal.window.entities.response.detail.couldn.t.unhide.entity = non è stato possibile mostrare di nuovo l'entità

journal.window.entities.response.detail.day.is.required = il giorno è obbligatorio

journal.window.entities.response.detail.description.is.required = la descrizione è obbligatoria

journal.window.entities.response.detail.detected.entity.activity.is.unavailable = l'attività dell'entità rilevata non è disponibile

journal.window.entities.response.detail.detected.entity.not.found = entità rilevata non trovata

journal.window.entities.response.detail.detected.entity.save.failed = salvataggio dell'entità rilevata non riuscito

journal.window.entities.response.detail.detected.entity.update.failed = aggiornamento dell'entità rilevata non riuscito

journal.window.entities.response.detail.detected.read.failed = lettura del rilevato non riuscita

journal.window.entities.response.detail.detections.can.t.be.shown.here.which.names.are = i rilevamenti non si possono mostrare qui: non si può leggere quali nomi sono nascosti

journal.window.entities.response.detail.entity.alias.update.failed = aggiornamento dell'alias dell'entità non riuscito

journal.window.entities.response.detail.entity.already.exists = l'entità esiste già

journal.window.entities.response.detail.entity.already.exists.in.destination.facet.use.merge.to = L'entità esiste già nella facetta di destinazione. Usa { $slot1 } per unire.

journal.window.entities.response.detail.entity.busy = entità occupata

journal.window.entities.response.detail.entity.create.failed = creazione dell'entità non riuscita

journal.window.entities.response.detail.entity.data.directory.not.found.in.source.facet = Cartella dei dati dell'entità non trovata nella facetta di origine.

journal.window.entities.response.detail.entity.description.update.failed = aggiornamento della descrizione dell'entità non riuscito

journal.window.entities.response.detail.entity.detach.failed = separazione dell'entità non riuscita

journal.window.entities.response.detail.entity.hidden = entità nascosta

journal.window.entities.response.detail.entity.id.is.required = { $slot1 } è obbligatorio

journal.window.entities.response.detail.entity.is.required = l'entità è obbligatoria

journal.window.entities.response.detail.entity.move.failed = spostamento dell'entità non riuscito

journal.window.entities.response.detail.entity.name.is.invalid = il nome dell'entità non è valido

journal.window.entities.response.detail.entity.not.found = entità non trovata

journal.window.entities.response.detail.entity.not.found.in.facet = Entità non trovata nella facetta

journal.window.entities.response.detail.entity.not.found.variant2 = Entità non trovata

journal.window.entities.response.detail.entity.read.failed = lettura dell'entità non riuscita

journal.window.entities.response.detail.entity.restore.failed = recupero dell'entità non riuscito

journal.window.entities.response.detail.entity.search.index.is.busy = l'indice di ricerca delle entità è occupato

journal.window.entities.response.detail.entity.search.index.is.stale = l'indice di ricerca delle entità non è aggiornato

journal.window.entities.response.detail.entity.search.index.is.unavailable = l'indice di ricerca delle entità non è disponibile

journal.window.entities.response.detail.entity.update.failed = aggiornamento dell'entità non riuscito

journal.window.entities.response.detail.entity.with.this.name.already.exists.in.facet = Un'entità con questo nome esiste già nella facetta

journal.window.entities.response.detail.evidence.is.required = la prova è obbligatoria

journal.window.entities.response.detail.exclude.name.is.required = { $slot1 } è obbligatorio

journal.window.entities.response.detail.facet.is.required = la facetta è obbligatoria

journal.window.entities.response.detail.facet.not.found = facetta non trovata

journal.window.entities.response.detail.facet.read.failed = lettura della facetta non riuscita

journal.window.entities.response.detail.from.facet.is.required = { $slot1 } è obbligatorio

journal.window.entities.response.detail.grid.read.failed = lettura della griglia non riuscita

journal.window.entities.response.detail.group.resolve.failed = risoluzione del gruppo non riuscita

journal.window.entities.response.detail.hidden.must.be.include.exclude.or.only = { $slot1 } deve essere { $slot2 }, { $slot3 } o { $slot4 }

journal.window.entities.response.detail.history.read.failed = lettura della cronologia non riuscita

journal.window.entities.response.detail.history.version.was.not.found = la versione della cronologia non è stata trovata

journal.window.entities.response.detail.ids.elements.must.be.strings = gli elementi di ids devono essere stringhe

journal.window.entities.response.detail.ids.is.required = ids è obbligatorio

journal.window.entities.response.detail.ids.must.be.an.array = ids deve essere un array

journal.window.entities.response.detail.index.plate.task.failed = operazione index plate non riuscita

journal.window.entities.response.detail.invalid.detected.entity = entità rilevata non valida

journal.window.entities.response.detail.invalid.target.kind = tipo di destinazione non valido

journal.window.entities.response.detail.journal.read.failed = lettura del diario non riuscita

journal.window.entities.response.detail.limit.bounded.two.hundred = il limite deve essere tra 0 e 200

journal.window.entities.response.detail.limit.is.required = il limite è obbligatorio

journal.window.entities.response.detail.limit.must.be.an.integer = il limite deve essere un intero

journal.window.entities.response.detail.member.ids.is.required = { $slot1 } è obbligatorio

journal.window.entities.response.detail.member.ids.must.contain.only.strings = { $slot1 } deve contenere solo stringhe

journal.window.entities.response.detail.member.ids.must.not.be.empty = { $slot1 } non deve essere vuoto

journal.window.entities.response.detail.merge.candidate.accept.failed = non è stato possibile accettare la candidata all'unione

journal.window.entities.response.detail.merge.candidate.dismiss.failed = non è stato possibile ignorare la candidata all'unione

journal.window.entities.response.detail.merge.candidate.record.failed = registro della candidata all'unione non riuscito

journal.window.entities.response.detail.merge.commit.failed = conferma dell'unione non riuscita

journal.window.entities.response.detail.merge.preview.failed = anteprima dell'unione non riuscita

journal.window.entities.response.detail.missing.name.key = Manca { $slot1 }

journal.window.entities.response.detail.name.is.required = il nome è obbligatorio

journal.window.entities.response.detail.no.data.provided = Nessun dato fornito

journal.window.entities.response.detail.observation.content.cannot.be.empty = il contenuto dell'osservazione non può essere vuoto

journal.window.entities.response.detail.observation.entity.lookup.failed = ricerca dell'entità dell'osservazione non riuscita

journal.window.entities.response.detail.offset.must.be.a.non.negative.integer = offset deve essere un intero non negativo

journal.window.entities.response.detail.offset.must.not.be.negative = offset non deve essere negativo

journal.window.entities.response.detail.old.name.and.new.name.are.required = { $slot1 } e { $slot2 } sono obbligatori

journal.window.entities.response.detail.operation.is.required = l'operazione è obbligatoria

journal.window.entities.response.detail.operation.must.be.a.string = l'operazione deve essere una stringa

journal.window.entities.response.detail.recorded.entity.choice.is.absent.from.this.facet = la scelta di entità conservata è assente da questa facetta

journal.window.entities.response.detail.recorded.entity.choice.is.hidden = la scelta di entità conservata è nascosta

journal.window.entities.response.detail.request.body.could.not.be.read = non è stato possibile leggere il corpo della richiesta

journal.window.entities.response.detail.resolution.task.failed = operazione di risoluzione non riuscita

journal.window.entities.response.detail.restore.is.not.available = il ripristino non è disponibile

journal.window.entities.response.detail.review.item.not.found = elemento di revisione non trovato

journal.window.entities.response.detail.review.read.failed = lettura della revisione non riuscita

journal.window.entities.response.detail.review.restore.failed = ripristino della revisione non riuscito

journal.window.entities.response.detail.review.sweep.failed = scansione della revisione non riuscita

journal.window.entities.response.detail.revision.is.required = la revisione è obbligatoria

journal.window.entities.response.detail.since.must.be.a.yyyymmdd.day = since deve essere un giorno { $slot1 }

journal.window.entities.response.detail.sort.must.be.activity.or.name = { $slot1 } deve essere { $slot2 } o { $slot3 }

journal.window.entities.response.detail.source.and.target.resolve.to.the.same.entity = fonte e destinazione si risolvono nella stessa entità.

journal.window.entities.response.detail.source.is.required = la fonte è obbligatoria

journal.window.entities.response.detail.source.slug.is.required = { $slot1 } è obbligatorio

journal.window.entities.response.detail.status.must.be.open.or.resolved = { $slot1 } deve essere { $slot2 } o { $slot3 }

journal.window.entities.response.detail.suggestions.are.busy.try.again = i suggerimenti sono occupati; riprova

journal.window.entities.response.detail.target.is.required = la destinazione è obbligatoria

journal.window.entities.response.detail.target.slug.is.required = { $slot1 } è obbligatorio

journal.window.entities.response.detail.the.name.couldn.t.be.changed.so.nothing.changed = non è stato possibile cambiare il nome, quindi non è cambiato niente

journal.window.entities.response.detail.the.names.hidden.in.this.facet.can.t.be = i nomi nascosti in questa facetta non si possono leggere, quindi non è cambiato niente

journal.window.entities.response.detail.this.entity.is.hidden.so.its.names.can.t = questa entità è nascosta, quindi i suoi nomi non possono cambiare finché non viene mostrata di nuovo

journal.window.entities.response.detail.this.entity.is.hidden.unhide.it.in.the.entities = Questa entità è nascosta. Mostrala di nuovo nell'app delle entità prima.

journal.window.entities.response.detail.to.facet.is.required = { $slot1 } è obbligatorio

journal.window.entities.response.detail.type.and.name.are.required = Tipo e nome sono obbligatori

journal.window.entities.response.detail.type.is.required = il tipo è obbligatorio

journal.window.entities.response.detail.type.must.be.a.canonical.entity.type.or.other = il tipo deve essere un tipo di entità canonico o altro

journal.window.entities.response.detail.version.id.is.required = { $slot1 } è obbligatorio

journal.window.entities.response.entity.exists = L'entità '{ $entity_name }' esiste già

journal.window.entities.response.entity.merged.permanent = '{ $merged_id }' è stata unita a '{ $survivor_id }'. Le unioni non si possono annullare, quindi quel nome non si può aggiungere come nuova entità; usa '{ $survivor_id }' al suo posto.

journal.window.entities.response.entity.missing = Entità '{ $entity_id }' non trovata

journal.window.entities.response.entity.type.invalid = Tipo di entità non valido { $entity_type }

journal.window.entities.response.entity.type.invalid.query = Tipo di entità non valido: { $entity_type }

journal.window.entities.response.entity.type.invalid.write = Tipo di entità non valido '{ $entity_type }'

journal.window.entities.response.facet.entity.missing = '{ $entity_name }' non è in questa facetta. aggiungila qui prima, poi aggiungi note su di essa.

journal.window.entities.response.ids.count.invalid = conteggio degli id non valido: { $id_count }

journal.window.entities.response.merge.refusal.both.principal = impossibile unire queste: sono entrambe segnate come te.

journal.window.entities.response.merge.refusal.person.type = impossibile unire una persona a qualcosa che non è una persona.

journal.window.entities.response.merge.refusal.principal.type = impossibile unire te a qualcosa che non è una persona.

journal.window.entities.response.merge.refusal.same.entity = Fonte e destinazione devono essere entità diverse.

journal.window.entities.response.merge.refusal.source.hidden = impossibile unire qualcosa che è nascosto. mostralo di nuovo prima.

journal.window.entities.response.merge.refusal.source.missing = Entità fonte non trovata:{ $slot1 }{ $id }

journal.window.entities.response.merge.refusal.target.hidden = impossibile unire a qualcosa che è nascosto. mostralo di nuovo prima.

journal.window.entities.response.merge.refusal.target.missing = Entità destinazione non trovata:{ $slot1 }{ $id }

journal.window.entities.response.merge.repair = Contatta un operatore per riparare questa unione.

journal.window.entities.response.operation.already.undone = l'operazione { $operation_id } è già stata annullata

journal.window.entities.response.operation.id.invalid = id dell'operazione non valido: { $operation_id }

journal.window.entities.response.operation.limit.invalid = limite non valido: { $limit }

journal.window.entities.response.photo.entity.not.found = non è stato possibile trovare quella persona.

journal.window.entities.response.photo.face.action.changed = questo è cambiato da quando la pagina è stata caricata. ricarica la pagina.

journal.window.entities.response.photo.face.not.found = quella foto non è più disponibile.

journal.window.entities.response.photo.faces.busy = i dati del volto sono occupati. riprova tra poco.

journal.window.entities.response.photo.faces.disabled = attiva i suggerimenti per la foto del profilo prima di cambiare una foto del profilo.

journal.window.entities.response.photo.faces.enabled = disattiva i suggerimenti per la foto del profilo prima di cancellare i dati del volto.

journal.window.entities.response.photo.faces.read.failed = non è stato possibile leggere quelle foto. riprova.

journal.window.entities.response.photo.request.invalid = non è stato possibile leggere quella richiesta di foto.

journal.window.entities.response.principal.protected = l'entità principale è protetta da questa operazione: { $entity_id }

journal.window.entities.response.query.integer.bounded = { $parameter_name } deve stare tra 0 e { $maximum }

journal.window.entities.response.query.integer.required = { $parameter_name } deve essere un intero

journal.window.entities.response.refused = Richiesta di entità rifiutata

journal.window.entities.response.summary.limit.bounded = il limite deve stare tra 0 e { $maximum }

journal.window.entities.scope.showing.facet = mostrando { $facet_name } ⌄

journal.window.entities.scope.showing.journal = mostrando tutto il tuo diario ⌄

journal.window.entities.search.finding.names = ricerca delle entità corrispondenti a "{ $query }"…

journal.window.entities.search.finding.type = ricerca delle entità { $entity_type }…

journal.window.entities.search.finding.type.and.names = ricerca delle entità { $entity_type } corrispondenti a "{ $query }"…

journal.window.entities.search.none.match = nessuna entità corrisponde a "{ $query }".

journal.window.entities.search.placeholder.entity.count = { $total_quantity ->
    [one] cerca { $total } entità
    [many] cerca { $total } di entità
   *[other] cerca { $total } entità
    }

journal.window.entities.section.heading.detach = separa

journal.window.entities.section.heading.no.entities.yet = ancora nessuna entità

journal.window.entities.state.edit.loading = caricamento…

journal.window.entities.state.edit.unavailable = non disponibile

journal.window.entities.text.couldn.t.hide.them = non è stato possibile nasconderle.

journal.window.entities.text.hiding.stopped.partway.undo.puts.back = il nascondere si è fermato a metà. annulla rimette tutto ciò che ha nascosto.

journal.window.entities.text.loading = caricamento…

journal.window.entities.text.loading.entities = caricamento delle entità…

journal.window.entities.text.undo.stopped.before.putting.any.back = l'annullamento si è fermato prima di rimetterne qualcuna. sono ancora sotto nascosto.

journal.window.entities.tidy.duplicate.count = { $pairs_quantity ->
    [one] { $pairs } possibile duplicato
    [many] { $pairs } di possibili duplicati
   *[other] { $pairs } possibili duplicati
    }

journal.window.entities.tidy.group.load.error = non è stato possibile caricare questo gruppo.

journal.window.entities.tidy.id.count = { $entities_quantity ->
    [one] { $entities } sembra un ID
    [many] { $entities } sembrano un ID
   *[other] { $entities } sembrano un ID
    }

journal.window.entities.title.couldn.t.erase.those = non è stato possibile cancellarle

journal.window.entities.title.couldn.t.move.that.to.the = non è stato possibile spostarla nel cestino

journal.window.entities.title.couldn.t.move.those.to.the = non è stato possibile spostarle nel cestino

journal.window.entities.title.couldn.t.recover.those = non è stato possibile recuperarle

journal.window.entities.title.delete.this.entity = elimina questa entità?

journal.window.entities.title.erase.this.entity.now = cancella questa entità ora?

journal.window.entities.title.moved.to.the.trash = spostata nel cestino

journal.window.entities.title.your.journal.didn.t.answer = il tuo diario non ha risposto

journal.window.entities.tooltip.add.to.facet = Aggiungi alla facetta

journal.window.entities.tooltip.cannot.hide.yourself = impossibile nasconderti

journal.window.entities.tooltip.entity.is.hidden.and.detached.from = l'entità è nascosta e separata da questa facetta

journal.window.entities.tooltip.entity.type = tipo: { $entity_type }

journal.window.entities.tooltip.facet.name = facetta: { $facet_name }

journal.window.entities.tooltip.has.voiceprint = Ha un'impronta vocale

journal.window.entities.tooltip.last.mentioned.full.day = Ultima menzione: { $day }

journal.window.entities.tooltip.last.mentioned.short.day = ultima menzione { $day }

journal.window.entities.tooltip.mention.days = { $mentions_quantity ->
    [one] { $mentions } menzione: giorni rilevati negli ultimi 30 giorni
    [many] { $mentions } di menzioni: giorni rilevati negli ultimi 30 giorni
   *[other] { $mentions } menzioni: giorni rilevati negli ultimi 30 giorni
    }

journal.window.entities.tooltip.this.entity.is.hidden = questa entità è nascosta

journal.window.entities.tooltip.this.is.you = questo sei tu

journal.window.entities.tooltip.this.is.your.entity.in.this = Questa è la tua entità in questa facetta

journal.window.entities.tooltip.unhide.this.entity = mostra di nuovo questa entità

journal.window.entities.tooltip.view.in.facet = Clicca per vedere in { $facet_name }

journal.window.entities.trust.history.change = { $previous_identity } → { $updated_identity }

journal.window.entities.trust.history.event = { $event_kind } · { $event_time } · { $entity_name }

journal.window.entities.trust.history.suggested.by = suggerito da { $agent_name }

journal.window.entities.trust.identity.details = { $entity_name } · { $entity_type } · alias: { $aliases }

journal.window.entities.trust.merge.confirm = unisci { $source_name } a { $target_name }? un'unione non si può annullare.

journal.window.entities.trust.merge.preview.counts = { $voice_samples_quantity ->
    [one] campioni di voce aggiunti: { $voice_samples } · campioni dopo l'unione: { $voice_total_quantity ->
        [one] { $voice_total }
        [many] { $voice_total }
       *[other] { $voice_total }
    } · note sotto il nome che si unisce: { $notes_quantity ->
        [one] { $notes }
        [many] { $notes }
       *[other] { $notes }
    } · etichette del parlante cambiate: { $speaker_labels_quantity ->
        [one] { $speaker_labels }
        [many] { $speaker_labels }
       *[other] { $speaker_labels }
    } · registri delle attività cambiati: { $activities_quantity ->
        [one] { $activities }
        [many] { $activities }
       *[other] { $activities }
    }
    [many] campioni di voce aggiunti: { $voice_samples } · campioni dopo l'unione: { $voice_total_quantity ->
        [one] { $voice_total }
        [many] { $voice_total }
       *[other] { $voice_total }
    } · note sotto il nome che si unisce: { $notes_quantity ->
        [one] { $notes }
        [many] { $notes }
       *[other] { $notes }
    } · etichette del parlante cambiate: { $speaker_labels_quantity ->
        [one] { $speaker_labels }
        [many] { $speaker_labels }
       *[other] { $speaker_labels }
    } · registri delle attività cambiati: { $activities_quantity ->
        [one] { $activities }
        [many] { $activities }
       *[other] { $activities }
    }
   *[other] campioni di voce aggiunti: { $voice_samples } · campioni dopo l'unione: { $voice_total_quantity ->
        [one] { $voice_total }
        [many] { $voice_total }
       *[other] { $voice_total }
    } · note sotto il nome che si unisce: { $notes_quantity ->
        [one] { $notes }
        [many] { $notes }
       *[other] { $notes }
    } · etichette del parlante cambiate: { $speaker_labels_quantity ->
        [one] { $speaker_labels }
        [many] { $speaker_labels }
       *[other] { $speaker_labels }
    } · registri delle attività cambiati: { $activities_quantity ->
        [one] { $activities }
        [many] { $activities }
       *[other] { $activities }
    }
    }

journal.window.entities.trust.merge.preview.summary = { $aliases_quantity ->
    [one] alias aggiunti: { $aliases } · email aggiunte: { $emails_quantity ->
        [one] { $emails }
        [many] { $emails }
       *[other] { $emails }
    }
    [many] alias aggiunti: { $aliases } · email aggiunte: { $emails_quantity ->
        [one] { $emails }
        [many] { $emails }
       *[other] { $emails }
    }
   *[other] alias aggiunti: { $aliases } · email aggiunte: { $emails_quantity ->
        [one] { $emails }
        [many] { $emails }
       *[other] { $emails }
    }
    }

journal.window.entities.trust.suggestion.details = { $source_name } → { $target_name } · { $facet_name }

journal.window.entities.visible.text.attach.to = allega a

journal.window.entities.visible.text.attach.to.facet = allega alla facetta

journal.window.entities.visible.text.attaching = allegamento in corso…

journal.window.entities.visible.text.cancel = annulla

journal.window.entities.visible.text.clear.selection = azzera la selezione

journal.window.entities.visible.text.comma.separated.list.of.alternative.names = elenco di nomi alternativi separati da virgole per la trascrizione audio

journal.window.entities.visible.text.connections = connessioni

journal.window.entities.visible.text.couldn.t.load.that.page.of = non è stato possibile caricare quella pagina di entità.

journal.window.entities.visible.text.description = descrizione

journal.window.entities.visible.text.detach = separa

journal.window.entities.visible.text.detached = separata

journal.window.entities.visible.text.detected.entities = entità rilevate

journal.window.entities.visible.text.dots.are.the.facets.a.name = i pallini sono le facette in cui compare un nome.

journal.window.entities.visible.text.each.pair.below.becomes.one.entity = ogni coppia qui sotto diventa un'entità. un'unione non si può annullare.

journal.window.entities.visible.text.edit.entity = modifica l'entità

journal.window.entities.visible.text.entities = entità

journal.window.entities.visible.text.facets = facette

journal.window.entities.visible.text.failed.to.save.changes = non è stato possibile salvare le modifiche

journal.window.entities.visible.text.found.by.ai.in.the.last = trovate dall'IA negli ultimi 30 giorni. segna con una stella per aggiungere al tuo diario

journal.window.entities.visible.text.has.voiceprint = Ha un'impronta vocale

journal.window.entities.visible.text.hidden = nascosto

journal.window.entities.visible.text.hide = nascondi

journal.window.entities.visible.text.hiding = nascondimento in corso…

journal.window.entities.visible.text.how.does.this.entity.relate.to = (come si rapporta questa entità a questa facetta?)

journal.window.entities.visible.text.loading = caricamento…

journal.window.entities.visible.text.loading.entities = caricamento delle entità…

journal.window.entities.visible.text.loading.this.entity = caricamento di questa entità…

journal.window.entities.visible.text.more.about.hidden = altro su nascosto

journal.window.entities.visible.text.name.is.required = il nome è obbligatorio

journal.window.entities.visible.text.names.shaped.like.codes.file.names = nomi che sembrano codici, nomi di file o numeri di richiesta. nessuno ha una nota.

journal.window.entities.visible.text.no.description.click.to.add = nessuna descrizione. clicca per aggiungere…

journal.window.entities.visible.text.no.entities.match.your.search = nessuna entità corrisponde alla tua ricerca.

journal.window.entities.visible.text.no.entity.matches.that.name = nessuna entità corrisponde a quel nome.

journal.window.entities.visible.text.not.attached = non allegata

journal.window.entities.visible.text.not.attached.to.any.facets.yet = non ancora allegata a nessuna facetta.

journal.window.entities.visible.text.not.now = non ora

journal.window.entities.visible.text.nothing.here = qui non c'è niente.

journal.window.entities.visible.text.only.in.facets.you.muted = solo nelle facette che hai silenziato.

journal.window.entities.visible.text.pairs.that.might.be.the.same = coppie che potrebbero essere la stessa persona o la stessa cosa. un'unione non si può annullare.

journal.window.entities.visible.text.people.places.and.things.you.re = persone, luoghi e cose di cui ti occupi

journal.window.entities.visible.text.review = rivedi

journal.window.entities.visible.text.review.the.group = rivedi il gruppo

journal.window.entities.visible.text.save = salva

journal.window.entities.visible.text.save.changes = salva le modifiche

journal.window.entities.visible.text.saved = salvato!

journal.window.entities.visible.text.saving = salvataggio in corso…

journal.window.entities.visible.text.select.a.facet.to.add.entities = Seleziona una facetta per aggiungere entità al tuo diario.

journal.window.entities.visible.text.still.working = ancora in corso…

journal.window.entities.visible.text.they.move.to.hidden.where.you = vanno in nascosto, dove puoi mostrarle di nuovo. gli agenti connessi non possono raggiungerle. la tua stessa entità non è mai in questo gruppo.

journal.window.entities.visible.text.try.again = riprova

journal.window.entities.visible.text.undo = annulla

journal.window.entities.visible.text.unhide = mostra di nuovo

journal.window.entities.visible.text.unhiding = mostra di nuovo in corso…

journal.window.entities.visible.text.unticked.ones.stay.as.they.are = quelli non spuntati restano come sono.

journal.window.entities.visible.text.worth.a.look = vale uno sguardo

journal.window.entities.visible.text.you = tu

journal.window.entities.voice.heading.count = { $samples_quantity ->
    [one] campioni di voce ({ $samples })
    [many] campioni di voce ({ $samples })
   *[other] campioni di voce ({ $samples })
    }

journal.window.entities.voice.invalid.key = chiave del campione di voce non valida

journal.window.entities.voice.sample.sentence = frase { $sentence_id }

journal.window.entities.window.explanation.couldn.t.keep.it.separate.it = non è stato possibile tenerla separata. è ancora elencata.

journal.window.entities.window.explanation.couldn.t.keep.them.separate.they = non è stato possibile tenerle separate. sono ancora elencate.

journal.window.entities.window.explanation.couldn.t.merge.it.it.s = non è stato possibile unirla. è ancora elencata.

journal.window.entities.window.explanation.couldn.t.merge.them.they.re = non è stato possibile unirle. sono ancora elencate.

journal.window.entities.window.explanation.couldn.t.search.for.a.merge = non è stato possibile cercare una destinazione per l'unione.

journal.window.entities.window.explanation.failed.to.load.observations = non è stato possibile caricare le osservazioni

journal.window.entities.window.explanation.look.like.an.id = sembrano un ID

journal.window.entities.window.explanation.this.entity.couldn.t.be.loaded = non è stato possibile caricare questa entità, quindi non si può modificare.

journal.window.entities.window.state.text.couldn.t.attach.entity = non è stato possibile associare l'entità

journal.window.entities.window.state.text.couldn.t.detach.entity = non è stato possibile separare l'entità

journal.window.entities.window.state.text.couldn.t.hide.entity = non è stato possibile nascondere l'entità

journal.window.entities.window.state.text.couldn.t.re.attach.entity = non è stato possibile associare di nuovo l'entità

journal.window.entities.window.state.text.couldn.t.save.description = non è stato possibile salvare la descrizione

journal.window.entities.window.state.text.couldn.t.unhide.entity = non è stato possibile mostrare di nuovo l'entità

journal.window.entities.window.state.text.failed.to.attach = Non è stato possibile associare

journal.window.entities.window.state.text.failed.to.attach.variant2 = non è stato possibile associare

journal.window.entities.window.state.text.failed.to.detach = Non è stato possibile separare

journal.window.entities.window.state.text.failed.to.re.attach = Non è stato possibile associare di nuovo

journal.window.entities.window.state.text.failed.to.save = Non è stato possibile salvare

journal.window.entities.window.state.text.finding.entities = ricerca delle entità…

journal.window.entities.window.state.text.loading.this.entity = caricamento di questa entità…

journal.window.entities.window.state.text.next.page = pagina successiva

journal.window.entities.window.state.text.not.set = non impostato

journal.window.entities.window.state.text.previous.page = pagina precedente

journal.window.entities.window.state.text.suggested.match = corrispondenza suggerita

journal.window.entities.window.state.text.try.again = riprova.

journal.window.facets.activities.calendar.scheduled.title = sul tuo calendario allora: { $meeting_title }

journal.window.facets.activities.copy.action.correct = non è corretto

journal.window.facets.activities.copy.action.edit = modifica

journal.window.facets.activities.copy.action.hide = nascondi

journal.window.facets.activities.copy.action.separate = separa

journal.window.facets.activities.copy.action.separate.yes = separala

journal.window.facets.activities.copy.action.unhide = mostra di nuovo

journal.window.facets.activities.copy.back = indietro

journal.window.facets.activities.copy.cancel = annulla

journal.window.facets.activities.copy.correct.title = non è corretto

journal.window.facets.activities.copy.correct.what = cos'è successo davvero

journal.window.facets.activities.copy.correct.who = chi l'ha detto

journal.window.facets.activities.copy.corrected.by.you = corretto da te

journal.window.facets.activities.copy.cue.edited = modificato da te

journal.window.facets.activities.copy.empty.facet.show.all = mostra tutte le facette

journal.window.facets.activities.copy.empty.today.heading = ancora niente oggi

journal.window.facets.activities.copy.evidence.open = apri nelle trascrizioni ›

journal.window.facets.activities.copy.evidence.open.narrow = apri ›

journal.window.facets.activities.copy.heading.accounts = che cosa l'ha conservata

journal.window.facets.activities.copy.heading.description = descrizione

journal.window.facets.activities.copy.heading.history = cronologia

journal.window.facets.activities.copy.heading.summary = riepilogo

journal.window.facets.activities.copy.hidden.outline = nascosto

journal.window.facets.activities.copy.history.remove = rimuovi

journal.window.facets.activities.copy.history.removed = rimosso

journal.window.facets.activities.copy.history.you.hid = l'hai nascosta

journal.window.facets.activities.copy.history.you.unhid = l'hai mostrata di nuovo

journal.window.facets.activities.copy.home.today.link = tutte le attività di oggi →

journal.window.facets.activities.copy.label.description = descrizione

journal.window.facets.activities.copy.label.title = titolo

journal.window.facets.activities.copy.lines.loading = ricerca delle righe pronunciate…

journal.window.facets.activities.copy.lines.only = solo queste righe

journal.window.facets.activities.copy.lines.others = altre righe pronunciate

journal.window.facets.activities.copy.load.error = non è stato possibile caricare le attività

journal.window.facets.activities.copy.loading = caricamento delle attività…

journal.window.facets.activities.copy.now = ora

journal.window.facets.activities.copy.people.loading = caricamento delle persone nel tuo diario…

journal.window.facets.activities.copy.person.search = trova una persona

journal.window.facets.activities.copy.reason.other = nascosto

journal.window.facets.activities.copy.reason.owner = l'hai nascosto

journal.window.facets.activities.copy.save = salva

journal.window.facets.activities.copy.scope.all = mostra tutte le facette

journal.window.facets.activities.copy.scope.all.option = tutte le facette

journal.window.facets.activities.copy.sheet.close = chiudi

journal.window.facets.activities.copy.show.hidden = mostra i nascosti

journal.window.facets.activities.copy.source.unknown = fonte sconosciuta

journal.window.facets.activities.copy.speaker.me = io

journal.window.facets.activities.copy.speaker.person = una persona

journal.window.facets.activities.copy.state.removed = rimosso

journal.window.facets.activities.copy.state.removed.by.you = rimosso da te

journal.window.facets.activities.copy.stop.showing.hidden = smetti di mostrare i nascosti

journal.window.facets.activities.copy.unhide = mostra di nuovo

journal.window.facets.activities.copy.untimed.heading = senza un orario

journal.window.facets.activities.copy.updating = aggiornamento in corso…

journal.window.facets.activities.empty.facet.day = niente in { $facet_name } in questo giorno

journal.window.facets.activities.evidence.duration.minutes = { $quantity ->
    [one] { $count } min
    [many] { $count } min
   *[other] { $count } min
    }

journal.window.facets.activities.evidence.segments.mixed = { $quantity ->
    [one] 1 segmento da più di una fonte, { $time_range }
    [many] { $count } di segmenti da più di una fonte, { $time_range }
   *[other] { $count } segmenti da più di una fonte, { $time_range }
    }

journal.window.facets.activities.evidence.segments.named = { $quantity ->
    [one] 1 segmento da { $stream_name }, { $time_range }
    [many] { $count } di segmenti da { $stream_name }, { $time_range }
   *[other] { $count } segmenti da { $stream_name }, { $time_range }
    }

journal.window.facets.activities.evidence.segments.unknown = { $quantity ->
    [one] 1 segmento da una fonte sconosciuta, { $time_range }
    [many] { $count } di segmenti da una fonte sconosciuta, { $time_range }
   *[other] { $count } segmenti da una fonte sconosciuta, { $time_range }
    }

journal.window.facets.activities.evidence.show.all.count = { $quantity ->
    [one] mostra { $count } segmento
    [many] mostra tutti i { $count } di segmenti
   *[other] mostra tutti i { $count } segmenti
    }

journal.window.facets.activities.hidden.footer.count = { $quantity ->
    [one] { $count } nascosto in questo giorno
    [many] { $count } nascosto in questo giorno
   *[other] { $count } nascosto in questo giorno
    }

journal.window.facets.activities.hidden.showing.count = { $quantity ->
    [one] mostra { $count } nascosto
    [many] mostra { $count } nascosto
   *[other] mostra { $count } nascosto
    }

journal.window.facets.activities.history.changed.fields = { $field_group ->
        *[title] hai cambiato il titolo
         [description] hai cambiato la descrizione
         [details] hai cambiato il tuo racconto
         [title-description] hai cambiato il titolo e la descrizione
         [title-details] hai cambiato il titolo e il tuo racconto
         [description-details] hai cambiato la descrizione e il tuo racconto
         [title-description-details] hai cambiato il titolo, la descrizione e il tuo racconto
    }

journal.window.facets.activities.history.changed.unclassified = hai cambiato{ $slot1 }

journal.window.facets.activities.history.corrected.lines = { $quantity ->
    [one] hai corretto chi ha detto 1 riga
    [many] hai corretto chi ha detto { $count } di righe
   *[other] hai corretto chi ha detto { $count } righe
    }

journal.window.facets.activities.history.found.stream = trovata nei segmenti da { $stream_name }

journal.window.facets.activities.history.remove.description = rimuovi: { $recorded_change }

journal.window.facets.activities.history.suggested.agent = suggerita da { $agent }

journal.window.facets.activities.line.correction.help = la tua risposta è per le righe che spunti, non per le loro voci. per insegnare una voce al tuo diario, o per riordinare le voci che conosce, vai a { $speakers_start }parlanti{ $speakers_end }.

journal.window.facets.activities.line.drawer.count = { $quantity ->
    [one] 1 riga
    [many] { $count } di righe
   *[other] { $count } righe
    }

journal.window.facets.activities.line.play.time = riproduci la riga alle { $time }

journal.window.facets.activities.line.question.selected = { $quantity ->
    [one] chi ha detto la riga che hai spuntato?
    [many] chi ha detto le { $count } di righe che hai spuntato?
   *[other] chi ha detto le { $count } righe che hai spuntato?
    }

journal.window.facets.activities.months.short.apr = Apr

journal.window.facets.activities.months.short.aug = Ago

journal.window.facets.activities.months.short.dec = Dic

journal.window.facets.activities.months.short.feb = Feb

journal.window.facets.activities.months.short.jan = Gen

journal.window.facets.activities.months.short.jul = Lug

journal.window.facets.activities.months.short.jun = Giu

journal.window.facets.activities.months.short.mar = Mar

journal.window.facets.activities.months.short.may = Mag

journal.window.facets.activities.months.short.nov = Nov

journal.window.facets.activities.months.short.oct = Ott

journal.window.facets.activities.months.short.sep = Set

journal.window.facets.activities.people.recorded.more = { $quantity ->
    [one] con { $first_person }, { $second_person } e 1 persona in più
    [many] con { $first_person }, { $second_person } e { $others } di persone in più
   *[other] con { $first_person }, { $second_person } e { $others } persone in più
    }

journal.window.facets.activities.people.recorded.one = con { $person }

journal.window.facets.activities.people.recorded.two = con { $first_person } e { $second_person }

journal.window.facets.activities.place.recorded.then = { $device_name } era { $place } allora

journal.window.facets.activities.response.detail.disallowed.fields = la patch contiene campi non consentiti: { $field_list }

journal.window.facets.activities.response.detail.previous.answer.unreadable = la risposta che questa modifica ha sostituito non si può leggere: { $read_error }

journal.window.facets.activities.response.detail.segment.outside.activity = { $segment_path } non è un segmento di questa attività

journal.window.facets.activities.response.error.detail.activities.couldn.t.be.updated.right = non è stato possibile aggiornare le attività ora perché erano occupate. riprova tra un momento.

journal.window.facets.activities.response.error.detail.day.must.not.be.after.today = il giorno non deve essere dopo oggi

journal.window.facets.activities.response.error.detail.day.not.found = Giorno non trovato

journal.window.facets.activities.response.error.detail.each.line.names.its.stream.segment = ogni riga indica il proprio flusso, il proprio segmento, la propria fonte e sentence_id

journal.window.facets.activities.response.error.detail.facet.must.not.be.empty = la facetta non deve essere vuota

journal.window.facets.activities.response.error.detail.invalid.month.format.expected.yyyymm = Formato del mese non valido, atteso { $slot1 }

journal.window.facets.activities.response.error.detail.lines.must.be.a.list.of = le righe devono essere un elenco di righe

journal.window.facets.activities.response.error.detail.only.a.record.in.a.group = solo un registro in un gruppo di due o più può essere separato

journal.window.facets.activities.response.error.detail.patch.must.be.a.non.empty = la patch deve essere un oggetto non vuoto

journal.window.facets.activities.response.error.detail.patch.must.not.be.empty = la patch non deve essere vuota

journal.window.facets.activities.response.error.detail.playback.must.be.true.or.false = la riproduzione deve essere vero o falso

journal.window.facets.activities.response.error.detail.position.timestamp.and.actor.name.the = posizione, timestamp e attore nominano la modifica da rimuovere

journal.window.facets.activities.response.error.detail.removing.an.edit.needs.the.owner = rimuovere una modifica richiede il titolare

journal.window.facets.activities.response.error.detail.saying.who.said.a.line.needs = dire chi ha detto una riga richiede il titolare

journal.window.facets.activities.response.error.detail.separating.a.group.needs.the.owner = separare un gruppo richiede il titolare

journal.window.facets.activities.response.error.detail.source.must.be.user = la fonte deve essere { $slot1 }

journal.window.facets.activities.response.error.detail.speaker.is.kind.owner.kind.person = il parlante è { $slot1 }, { $slot2 } o { $slot3 }

journal.window.facets.activities.response.error.detail.title.must.not.be.empty = il titolo non deve essere vuoto

journal.window.facets.activities.response.error.message.activities.couldn.t.be.updated.right = non è stato possibile aggiornare le attività ora perché erano occupate. riprova tra un momento.

journal.window.facets.activities.response.error.message.only.you.can.remove.a.correction = solo tu puoi rimuovere una correzione.

journal.window.facets.activities.response.error.message.only.you.can.say.who.said = solo tu puoi dire chi ha detto queste righe.

journal.window.facets.activities.response.error.message.only.you.can.separate.a.group = solo tu puoi separare un gruppo.

journal.window.facets.activities.response.error.message.that.activity.couldn.t.be.created = non è stato possibile creare l'attività perché esiste già.

journal.window.facets.activities.response.error.message.that.activity.isn.t.in.the = quell'attività non è nella facetta.

journal.window.facets.activities.response.error.message.that.activity.setting.couldn.t.be = non è stato possibile usare quell'impostazione dell'attività.

journal.window.facets.activities.response.error.message.that.day.couldn.t.be.used = non è stato possibile usare quel giorno.

journal.window.facets.activities.response.error.message.that.edit.couldn.t.be.removed = non è stato possibile rimuovere quella modifica.

journal.window.facets.activities.response.error.message.that.month.couldn.t.be.used = non è stato possibile usare quel mese.

journal.window.facets.activities.response.error.message.that.request.didn.t.finish = quella richiesta non è terminata.

journal.window.facets.activities.response.error.message.this.isn.t.grouped.with.anything = questo non è in un gruppo con niente.

journal.window.facets.activities.response.error.message.this.record.is.being.moved.you = questo registro si sta spostando. puoi modificarlo quando lo spostamento è finito.

journal.window.facets.activities.response.error.message.who.said.those.lines.couldn.t = non è stato possibile salvare chi ha detto quelle righe.

journal.window.facets.activities.scope.selected.facet = mostra { $facet_name }

journal.window.facets.activities.timeline.item.hidden = { $time_range }, { $facet_name }, nascosto: { $activity_title }

journal.window.facets.activities.timeline.item.hidden.source = { $time_range }, { $facet_name }, { $stream_name }, nascosto: { $activity_title }

journal.window.facets.activities.timeline.item.visible = { $time_range }, { $facet_name }: { $activity_title }

journal.window.facets.activities.timeline.item.visible.source = { $time_range }, { $facet_name }, { $stream_name }: { $activity_title }

journal.window.facets.activities.window.explanation.a.line.shows.the.wrong.person = una riga mostra la persona sbagliata

journal.window.facets.activities.window.explanation.activities.appear.here.a.little.while = le attività compaiono qui poco dopo che finiscono, e possono ancora crescere man mano che ne arriva altro.

journal.window.facets.activities.window.explanation.added.with.the.solstone.command = aggiunta con il comando di solstone

journal.window.facets.activities.window.explanation.as.the.day.goes.on.activities = con il passare del giorno, le attività da ciò che condividi compaiono qui.

journal.window.facets.activities.window.explanation.can.t.be.read.right.now = non si può leggere ora

journal.window.facets.activities.window.explanation.changed.with.the.solstone.command = cambiata con il comando di solstone

journal.window.facets.activities.window.explanation.couldn.t.load.the.people.in = non è stato possibile caricare le persone nel tuo diario.

journal.window.facets.activities.window.explanation.couldn.t.remove.that.try.again = non è stato possibile rimuovere quello. riprova tra un momento.

journal.window.facets.activities.window.explanation.couldn.t.save.that.try.again = non è stato possibile salvare quello. riprova tra un momento.

journal.window.facets.activities.window.explanation.couldn.t.save.your.changes.are = non è stato possibile salvare. le tue modifiche sono ancora qui, quindi puoi riprovare.

journal.window.facets.activities.window.explanation.found.in.segments.from.an.unknown = trovata nei segmenti da una fonte sconosciuta

journal.window.facets.activities.window.explanation.found.in.segments.from.more.than = trovata nei segmenti da più di una fonte

journal.window.facets.activities.window.explanation.grew.as.more.of.it.came = è cresciuta man mano che ne arrivava altro

journal.window.facets.activities.window.explanation.hidden.it.s.still.in.your = nascosta. è ancora nel tuo diario.

journal.window.facets.activities.window.explanation.hidden.with.the.solstone.command = nascosta con il comando di solstone

journal.window.facets.activities.window.explanation.it.won.t.join.this.group = non si unirà più a questo gruppo. niente viene eliminato.

journal.window.facets.activities.window.explanation.lines.your.journal.credits.to.you = righe che il tuo diario attribuisce a te

journal.window.facets.activities.window.explanation.more.than.one.source = più di una fonte

journal.window.facets.activities.window.explanation.no.activities.on.this.day = nessuna attività in questo giorno

journal.window.facets.activities.window.explanation.no.longer.in.your.journal = non più nel tuo diario

journal.window.facets.activities.window.explanation.no.one.in.your.journal.matches = nessuna persona nel tuo diario corrisponde a quello.

journal.window.facets.activities.window.explanation.on.your.calendar.then = sul tuo calendario in quel momento

journal.window.facets.activities.window.explanation.other.facets.have.activities.on.this = altre facette hanno attività in questo giorno.

journal.window.facets.activities.window.explanation.part.of.it.removed = parte di essa rimossa

journal.window.facets.activities.window.explanation.pick.an.activity.to.see.what = scegli un'attività per vedere su cosa si basa.

journal.window.facets.activities.window.explanation.say.it.the.way.you.d = dillo come lo diresti a un amico: chi ha fatto cosa, cos'è stato deciso, cos'è ancora aperto.

journal.window.facets.activities.window.explanation.seen.as.cancelled.or.declined = vista come annullata o rifiutata

journal.window.facets.activities.window.explanation.some.of.those.lines.aren.t = alcune di quelle righe non fanno parte di questa attività.

journal.window.facets.activities.window.explanation.someone.i.don.t.know = qualcuno che non conosco

journal.window.facets.activities.window.explanation.tell.your.journal.in.your.own = di' al tuo diario con le tue parole

journal.window.facets.activities.window.explanation.that.changed.since.it.was.shown = è cambiata da quando è stata mostrata. ecco come sta ora.

journal.window.facets.activities.window.explanation.that.person.can.t.be.chosen = quella persona non può essere scelta. scegli qualcun altro, o qualcuno che non conosci.

journal.window.facets.activities.window.explanation.the.journal.adds.activities.as.it = il diario aggiunge attività mentre elabora ciò che hai condiviso.

journal.window.facets.activities.window.explanation.these.lines.can.t.take.an = queste righe non possono ricevere una risposta su chi le ha dette. puoi invece dire al tuo diario cos'è successo.

journal.window.facets.activities.window.explanation.these.lines.have.changed.since.they = queste righe sono cambiate da quando sono state mostrate. torna indietro e aprile di nuovo.

journal.window.facets.activities.window.explanation.this.was.a.recording.or.video = era una registrazione o un video in riproduzione

journal.window.facets.activities.window.explanation.tick.the.lines.you.want.to = spunta prima le righe che vuoi correggere.

journal.window.facets.activities.window.explanation.unhidden.with.the.solstone.command = mostrata di nuovo con il comando di solstone

journal.window.facets.activities.window.explanation.unhide.the.day.on.the.body = mostra di nuovo il giorno nella pagina del corpo

journal.window.facets.activities.window.explanation.what.it.s.based.on = su cosa si basa

journal.window.facets.activities.window.explanation.who.said.the.lines.you.ticked = chi ha detto le righe che hai spuntato?

journal.window.facets.activities.window.explanation.yes.it.was.playing = sì, era in riproduzione

journal.window.facets.activities.window.explanation.you.hid.this.day.s.body = hai nascosto i dati del corpo di questo giorno

journal.window.facets.activities.window.explanation.you.said.this.was.a.recording = hai detto che era una registrazione o un video in riproduzione

journal.window.facets.activities.window.explanation.you.told.your.journal.what.happened = hai detto al tuo diario cos'è successo

journal.window.facets.activities.window.explanation.your.account.of.it = il tuo racconto

journal.window.facets.activities.window.explanation.your.earlier.answers.about.who.spoke = le tue risposte precedenti su chi ha parlato qui non si possono leggere, quindi questa non è stata salvata.

journal.window.facets.activities.window.explanation.your.journal.doesn.t.know.which = il tuo diario non sa ancora quale persona sei, quindi non può segnare le righe come tue.

journal.window.facets.activities.window.explanation.your.journal.will.think.this.through = il tuo diario ci ragionerà di nuovo, come qualcosa che hai guardato o ascoltato, senza nessun altro che partecipi.

journal.window.facets.awareness.response.detail.multiple.import.answers = fornisci esattamente uno di { $slot1 }; ricevuto { $answer_list }

journal.window.facets.awareness.response.detail.unknown.section = nessuna sezione awareness chiamata '{ $section_name }'

journal.window.facets.awareness.response.detail.unreadable.settings = non è stato possibile leggere il tuo file delle impostazioni in { $settings_path }. le tue impostazioni non sono state cambiate. ripara il file o ripristina { $slot3 } da un backup, poi riprova.

journal.window.facets.awareness.response.error.detail.kind.is.required = il tipo è obbligatorio

journal.window.facets.awareness.response.error.detail.no.request.body = nessun corpo della richiesta

journal.window.facets.awareness.response.error.detail.request.body.must.be.a.json = il corpo della richiesta deve essere un oggetto { $slot1 }

journal.window.facets.awareness.response.error.message.a.required.field.is.missing = manca un campo obbligatorio.

journal.window.facets.awareness.response.error.message.one.of.those.values.couldn.t = non è stato possibile usare uno di quei valori.

journal.window.facets.awareness.response.error.message.that.json.request.couldn.t.be = non è stato possibile leggere quella richiesta { $slot1 }.

journal.window.facets.awareness.response.error.message.that.part.of.your.journal.couldn = non è stato possibile trovare quella parte del tuo diario.

journal.window.facets.awareness.response.error.message.that.request.didn.t.finish = quella richiesta non è terminata.

journal.window.facets.awareness.response.error.message.that.request.had.no.data.in = quella richiesta non conteneva dati.

journal.window.facets.awareness.response.error.message.the.awareness.operation.is.busy = L'operazione awareness è occupata.

journal.window.facets.awareness.response.error.message.your.settings.couldn.t.be.read = non è stato possibile leggere le impostazioni.

journal.window.facets.awareness.response.imports.busy.detail = le importazioni sono occupate; riprova

journal.window.facets.awareness.response.one.import.answer = fornisci esattamente uno di { $slot1 }

journal.window.facets.curation.ambiguity.choose.name = scegli { $candidate_name }

journal.window.facets.curation.ambiguity.member.occurrences = { $quantity ->
    [one] { $count } occorrenza
    [many] { $count } di occorrenze
   *[other] { $count } occorrenze
    }

journal.window.facets.curation.ambiguity.occurrences = { $quantity ->
    [one] { $count } occorrenza
    [many] { $count } di occorrenze
   *[other] { $count } occorrenze
    }

journal.window.facets.curation.ambiguity.question = quale elemento corrisponde a "{ $ambiguous_name }"?

journal.window.facets.curation.button.label.try.again = riprova

journal.window.facets.curation.disclosure.label.individual.questions.and.sources = domande e fonti individuali

journal.window.facets.curation.disclosure.label.source.evidence = prova della fonte

journal.window.facets.curation.disclosure.label.source.identifiers = identificatori della fonte

journal.window.facets.curation.disclosure.label.technical.details = dettagli tecnici

journal.window.facets.curation.evidence.drawer.title = prova

journal.window.facets.curation.evidence.showing.partial = { $shown_quantity ->
    [one] mostra { $shown } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
    [many] mostra { $shown } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
   *[other] mostra { $shown } di { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
    }

journal.window.facets.curation.evidence.total.pieces = { $quantity ->
    [one] 1 prova
    [many] { $count } di prove
   *[other] { $count } prove
    }

journal.window.facets.curation.facet.additional.names = compare anche come { $alias_list }.

journal.window.facets.curation.facet.create.question = l'attività del diario non si adatta bene alle tue facette. crea la facetta "{ $facet_name }"?

journal.window.facets.curation.list.items = { $preceding_items } e { $last_item }

journal.window.facets.curation.placeholder.this.is.a.speaker.label.not = questa è un'etichetta del parlante, non un nome

journal.window.facets.curation.preview.activities.count = { $quantity ->
    [one] registri delle attività cambiati: { $count }
    [many] registri delle attività cambiati: { $count }
   *[other] registri delle attività cambiati: { $count }
    }

journal.window.facets.curation.preview.aliases = alias aggiunti: { $aliases }

journal.window.facets.curation.preview.emails.count = { $quantity ->
    [one] email aggiunte: { $count }
    [many] email aggiunte: { $count }
   *[other] email aggiunte: { $count }
    }

journal.window.facets.curation.preview.facet.links = { $moved_quantity ->
    [one] collegamenti delle facette: { $moved } spostati, { $merged_quantity ->
        [one] { $merged } uniti
        [many] { $merged } uniti
       *[other] { $merged } uniti
    }
    [many] collegamenti delle facette: { $moved } spostati, { $merged_quantity ->
        [one] { $merged } uniti
        [many] { $merged } uniti
       *[other] { $merged } uniti
    }
   *[other] collegamenti delle facette: { $moved } spostati, { $merged_quantity ->
        [one] { $merged } uniti
        [many] { $merged } uniti
       *[other] { $merged } uniti
    }
    }

journal.window.facets.curation.preview.notes.count = { $quantity ->
    [one] note sotto il nome che si unisce: { $count }
    [many] note sotto il nome che si unisce: { $count }
   *[other] note sotto il nome che si unisce: { $count }
    }

journal.window.facets.curation.preview.observations.count = { $quantity ->
    [one] note spostate: { $count }
    [many] note spostate: { $count }
   *[other] note spostate: { $count }
    }

journal.window.facets.curation.preview.speaker.labels.count = { $quantity ->
    [one] etichette del parlante cambiate: { $count }
    [many] etichette del parlante cambiate: { $count }
   *[other] etichette del parlante cambiate: { $count }
    }

journal.window.facets.curation.preview.speaker.segments = { $labels_quantity ->
    [one] etichette del parlante aggiornate: { $labels } etichetta, { $corrections_quantity ->
        [one] { $corrections } correzione
        [many] { $corrections } di correzioni
       *[other] { $corrections } correzioni
    }
    [many] etichette del parlante aggiornate: { $labels } di etichette, { $corrections_quantity ->
        [one] { $corrections } correzione
        [many] { $corrections } di correzioni
       *[other] { $corrections } correzioni
    }
   *[other] etichette del parlante aggiornate: { $labels } etichette, { $corrections_quantity ->
        [one] { $corrections } correzione
        [many] { $corrections } di correzioni
       *[other] { $corrections } correzioni
    }
    }

journal.window.facets.curation.preview.voice.samples = { $added_quantity ->
    [one] campioni di voce spostati: { $added } aggiunti, { $total_quantity ->
        [one] { $total } totale
        [many] { $total } totale
       *[other] { $total } totale
    }
    [many] campioni di voce spostati: { $added } aggiunti, { $total_quantity ->
        [one] { $total } totale
        [many] { $total } totale
       *[other] { $total } totale
    }
   *[other] campioni di voce spostati: { $added } aggiunti, { $total_quantity ->
        [one] { $total } totale
        [many] { $total } totale
       *[other] { $total } totale
    }
    }

journal.window.facets.curation.response.batch.candidate.identity.missing = alla candidata manca facet, source_slug o target_slug

journal.window.facets.curation.response.candidate.absent = candidata non trovata

journal.window.facets.curation.response.candidate.accept.status = impossibile accettare la candidata con stato{ $slot1 }{ $status }

journal.window.facets.curation.response.candidate.dismiss.status = impossibile ignorare la candidata con stato{ $slot1 }{ $status }

journal.window.facets.curation.response.candidate.kept.name.missing = "{ $name }" è già unita a "{ $kept }", senza conservarne il nome. aggiungi "{ $name }" ai nomi alternativi di "{ $kept }" nella pagina delle entità, poi accetta di nuovo.

journal.window.facets.curation.response.candidate.preview.status = impossibile vedere l'anteprima della candidata con stato{ $slot1 }{ $status }

journal.window.facets.curation.response.copy.action.failed = non è andato a buon fine. riprova.

journal.window.facets.curation.response.copy.ambiguity.body = quale elemento corrisponde a "{ $slot1 }"?

journal.window.facets.curation.response.copy.ambiguity.choose.action = scegli { $slot1 }

journal.window.facets.curation.response.copy.ambiguity.dismiss.action = nessuno di questi

journal.window.facets.curation.response.copy.ambiguity.origin.label = notato in

journal.window.facets.curation.response.copy.empty.state = niente da rivedere. ancora nessuna struttura nuova da suggerire.

journal.window.facets.curation.response.copy.entity.batch.confirm.dismiss.action = tieni tutti separati

journal.window.facets.curation.response.copy.entity.batch.confirm.merge.action = unisci tutti

journal.window.facets.curation.response.copy.entity.batch.dismiss.action = tieni i selezionati separati

journal.window.facets.curation.response.copy.entity.batch.dismiss.lead = queste coppie saranno tenute separate:

journal.window.facets.curation.response.copy.entity.batch.dismiss.summary = { $slot1 } di { $slot2 } tenuti separati.

journal.window.facets.curation.response.copy.entity.batch.failed.note = { $slot1 } richiedono ancora attenzione qui sotto.

journal.window.facets.curation.response.copy.entity.batch.merge.action = unisci i selezionati

journal.window.facets.curation.response.copy.entity.batch.merge.lead = queste coppie saranno unite. le unioni non si possono annullare:

journal.window.facets.curation.response.copy.entity.batch.merge.summary = { $slot1 } di { $slot2 } uniti.

journal.window.facets.curation.response.copy.entity.body = "{ $slot1 }" e "{ $slot2 }" sembrano la stessa entità. uniscile?

journal.window.facets.curation.response.copy.entity.cancel.action = annulla

journal.window.facets.curation.response.copy.entity.confirm.action = conferma l'unione

journal.window.facets.curation.response.copy.entity.dismiss.action = tieni separati

journal.window.facets.curation.response.copy.entity.done.for.now.action = fatto per ora

journal.window.facets.curation.response.copy.entity.facets.label = (in { $slot1 })

journal.window.facets.curation.response.copy.entity.group.lede = queste { $slot1 } coppie si somigliano. unirle tiene i tuoi nomi a posto.

journal.window.facets.curation.response.copy.entity.merge.action = unisci

journal.window.facets.curation.response.copy.entity.preview.empty = non servono modifiche al diario per questa unione.

journal.window.facets.curation.response.copy.entity.preview.errors = alcuni aggiornamenti dei segmenti possono richiedere attenzione.

journal.window.facets.curation.response.copy.entity.preview.lead = prima di unire, ecco cosa cambierà. un'unione non si può annullare.

journal.window.facets.curation.response.copy.entity.progress.label = { $slot1 } rivisti · { $slot2 } rimasti

journal.window.facets.curation.response.copy.entity.select.all.action = seleziona tutto

journal.window.facets.curation.response.copy.entity.show.rest.action = mostra il resto ({ $slot1 })

journal.window.facets.curation.response.copy.facet.body = l'attività del diario non si adatta bene alle tue facette. crea la facetta "{ $slot1 }"?

journal.window.facets.curation.response.copy.facet.create.action = crea la facetta

journal.window.facets.curation.response.copy.facet.dismiss.action = non ora

journal.window.facets.curation.response.copy.heading = suggerimenti

journal.window.facets.curation.response.copy.preview.activities = registri delle attività cambiati

journal.window.facets.curation.response.copy.preview.akas.label = alias aggiunti

journal.window.facets.curation.response.copy.preview.emails.label = email aggiunte

journal.window.facets.curation.response.copy.preview.facets.label = collegamenti delle facette

journal.window.facets.curation.response.copy.preview.labels = etichette del parlante cambiate

journal.window.facets.curation.response.copy.preview.notes.source = note sotto il nome che si unisce

journal.window.facets.curation.response.copy.preview.observations.label = note spostate

journal.window.facets.curation.response.copy.preview.partial = l'unione unisce anche i collegamenti delle facette e le foto.

journal.window.facets.curation.response.copy.preview.segments.label = etichette del parlante aggiornate

journal.window.facets.curation.response.copy.preview.voiceprints.label = campioni di voce spostati

journal.window.facets.curation.response.copy.repair.required = { $slot1 } { $slot2 }

journal.window.facets.curation.response.copy.review.restore.action = riporta in revisione

journal.window.facets.curation.response.copy.speaker.body = "{ $slot1 }" e "{ $slot2 }" possono essere lo stesso parlante. uniscili?

journal.window.facets.curation.response.copy.speaker.candidate.pair.body = due voci nel tuo diario suonano simili. uniscile?

journal.window.facets.curation.response.copy.speaker.candidate.pair.dismiss.action = tieni separati

journal.window.facets.curation.response.copy.speaker.candidate.pair.intervals.label = intervalli

journal.window.facets.curation.response.copy.speaker.candidate.pair.merge.action = unisci le candidate

journal.window.facets.curation.response.copy.speaker.candidate.pair.similarity.label = coseno

journal.window.facets.curation.response.copy.speaker.candidate.pair.source.label = candidata A

journal.window.facets.curation.response.copy.speaker.candidate.pair.target.label = candidata B

journal.window.facets.curation.response.copy.speaker.dismiss.action = tieni separati

journal.window.facets.curation.response.copy.speaker.merge.action = rivedi l'unione

journal.window.facets.curation.response.detail.missing.field = Manca { $field_name }

journal.window.facets.curation.response.detail.unknown.group = gruppo di curation sconosciuto: { $group }

journal.window.facets.curation.response.detail.unreadable.settings = non è stato possibile leggere il tuo file delle impostazioni in { $settings_path }. le tue impostazioni non sono state cambiate. ripara il file o ripristina { $slot3 } da un backup, poi riprova.

journal.window.facets.curation.response.entity.busy.detail = le candidate all'unione delle entità sono occupate; riprova

journal.window.facets.curation.response.error.detail.key.does.not.match.anchor.a = la chiave non corrisponde a { $slot1 }

journal.window.facets.curation.response.error.detail.key.does.not.match.source.id = la chiave non corrisponde a { $slot1 }

journal.window.facets.curation.response.error.message.a.required.field.is.missing = manca un campo obbligatorio.

journal.window.facets.curation.response.error.message.one.of.those.values.couldn.t = non è stato possibile usare uno di quei valori.

journal.window.facets.curation.response.error.message.that.entity.operation.couldn.t.be = non è stato possibile completare quell'operazione sull'entità.

journal.window.facets.curation.response.error.message.that.list.isn.t.one.curation = quell'elenco non è uno che la curatela sfoglia.

journal.window.facets.curation.response.error.message.the.entity.operation.is.busy = L'operazione sull'entità è occupata.

journal.window.facets.curation.response.error.message.your.settings.couldn.t.be.read = non è stato possibile leggere le impostazioni.

journal.window.facets.curation.response.pair.already.confirmed = impossibile unire due candidate confermate

journal.window.facets.curation.response.pair.already.merged = la coppia di candidate è già unita

journal.window.facets.curation.response.pair.anchor.absent = ancora della candidata non trovata

journal.window.facets.curation.response.pair.below.threshold = la coppia di candidate è sotto la soglia di revisione

journal.window.facets.curation.response.pair.rejected = impossibile unire la candidata rifiutata

journal.window.facets.curation.response.speaker.busy.detail = i suggerimenti sul parlante sono occupati; riprova

journal.window.facets.curation.review.total = { $quantity ->
    [one] { $count } suggerimento da rivedere. scegli un gruppo per iniziare.
    [many] { $count } di suggerimenti da rivedere. scegli un gruppo per iniziare.
   *[other] { $count } suggerimenti da rivedere. scegli un gruppo per iniziare.
    }

journal.window.facets.curation.speaker.merge.question = "{ $source_name }" e "{ $target_name }" potrebbero essere lo stesso parlante. uniscili?

journal.window.facets.curation.status.text.journal = diario

journal.window.facets.curation.status.text.please.select.at.least.one.question = per favore seleziona almeno una domanda da risolvere.

journal.window.facets.curation.status.text.restore.to.review = riporta in revisione

journal.window.facets.curation.suggestion.display.text.another.change.is.in.progress.try = un'altra modifica è in corso. riprova tra poco.

journal.window.facets.curation.suggestion.display.text.automatic.review = revisione automatica

journal.window.facets.curation.suggestion.display.text.couldn.t.apply.that.choice.refresh = non è stato possibile applicare quella scelta. ricarica i suggerimenti e riprova.

journal.window.facets.curation.suggestion.display.text.couldn.t.save.that.change.try = non è stato possibile salvare quella modifica. riprova.

journal.window.facets.curation.suggestion.display.text.facet.suggestions = suggerimenti di facette

journal.window.facets.curation.suggestion.display.text.names.to.clarify = nomi da chiarire

journal.window.facets.curation.suggestion.display.text.possible.duplicate.voices = possibili voci duplicate

journal.window.facets.curation.suggestion.display.text.show.more = mostra altro

journal.window.facets.curation.suggestion.display.text.this.suggestion.or.one.of.its = questo suggerimento o una delle sue corrispondenze non è più disponibile. ricarica i suggerimenti.

journal.window.facets.curation.suggestions.accept = accetta

journal.window.facets.curation.suggestions.account.attributed.agent = il tuo racconto, come l'ha scritto { $agent_name }

journal.window.facets.curation.suggestions.basis.agent.journal.shows = { $agent_name } dice di averlo notato nel tuo diario

journal.window.facets.curation.suggestions.basis.agent.my.own.work = { $agent_name } dice di saperlo di prima mano

journal.window.facets.curation.suggestions.basis.agent.outside.source = { $agent_name } dice di averlo trovato fuori dal tuo diario

journal.window.facets.curation.suggestions.basis.agent.owner.said = { $agent_name } dice che glielo hai detto

journal.window.facets.curation.suggestions.cancel = annulla

journal.window.facets.curation.suggestions.dismiss = ignora

journal.window.facets.curation.suggestions.earlier.accepted = l'hai accettato

journal.window.facets.curation.suggestions.earlier.agent = un agente precedente

journal.window.facets.curation.suggestions.earlier.dismissed = l'hai ignorato

journal.window.facets.curation.suggestions.earlier.in.duplicates = nei possibili duplicati

journal.window.facets.curation.suggestions.earlier.not.accepted = non accettato

journal.window.facets.curation.suggestions.earlier.stale = già cambiato

journal.window.facets.curation.suggestions.edit = modifica

journal.window.facets.curation.suggestions.edit.name.label = nome

journal.window.facets.curation.suggestions.filter.empty.agent = niente da { $agent_name } è in attesa.

journal.window.facets.curation.suggestions.filter.only.agent = solo da { $agent_name }.

journal.window.facets.curation.suggestions.group = dai tuoi agenti

journal.window.facets.curation.suggestions.keep.name = nome da usare

journal.window.facets.curation.suggestions.kind.name = un nome

journal.window.facets.curation.suggestions.kind.same.person = la stessa persona

journal.window.facets.curation.suggestions.kind.what.happened = cos'è successo davvero

journal.window.facets.curation.suggestions.kind.who.said.it = chi l'ha detto

journal.window.facets.curation.suggestions.lines.assigned.named = { $quantity ->
    [one] questa riga è stata detta da { $speaker }
    [many] queste { $count } di righe sono state dette da { $speaker }
   *[other] queste { $count } righe sono state dette da { $speaker }
    }

journal.window.facets.curation.suggestions.lines.assigned.owner = { $quantity ->
    [one] questa riga è stata detta da { $speaker_start }te{ $speaker_end }
    [many] queste { $count } di righe sono state dette da { $speaker_start }te{ $speaker_end }
   *[other] queste { $count } righe sono state dette da { $speaker_start }te{ $speaker_end }
    }

journal.window.facets.curation.suggestions.lines.assigned.unknown = { $quantity ->
    [one] questa riga è stata detta da { $speaker_start }qualcuno che non conosci{ $speaker_end }
    [many] queste { $count } di righe sono state dette da { $speaker_start }qualcuno che non conosci{ $speaker_end }
   *[other] queste { $count } righe sono state dette da { $speaker_start }qualcuno che non conosci{ $speaker_end }
    }

journal.window.facets.curation.suggestions.name.alias = { $former_name }, noto anche come { $new_name }

journal.window.facets.curation.suggestions.name.change = { $former_name } → { $new_name }

journal.window.facets.curation.suggestions.now = il tuo diario dice

journal.window.facets.curation.suggestions.now.people = il tuo diario ha

journal.window.facets.curation.suggestions.ok = ok

journal.window.facets.curation.suggestions.open = apri

journal.window.facets.curation.suggestions.open.activity = apri l'attività

journal.window.facets.curation.suggestions.open.duplicates = apri i possibili duplicati

journal.window.facets.curation.suggestions.open.lines = apri le righe

journal.window.facets.curation.suggestions.open.person.name = apri { $person_name }

journal.window.facets.curation.suggestions.person.facet.membership = in { $facet_list }

journal.window.facets.curation.suggestions.person.notes.count = { $quantity ->
    [one] 1 nota
    [many] { $count } di note
   *[other] { $count } note
    }

journal.window.facets.curation.suggestions.receipt.accepted.alias = accettato il suggerimento di { $agent_name }. { $former_name } si chiama anche { $new_name } ora.

journal.window.facets.curation.suggestions.receipt.accepted.ask = accettato il suggerimento di { $agent_name }. il tuo diario ci ragionerà di nuovo.

journal.window.facets.curation.suggestions.receipt.accepted.ask.unconfirmed = accettato il suggerimento di { $agent_name } e salvato, ma il tuo diario non ha potuto confermare che ci ragionerà di nuovo.

journal.window.facets.curation.suggestions.receipt.accepted.merged.people = accettato il suggerimento di { $agent_name }. { $merged_name } e { $kept_name } ora sono una persona sola, di nome { $kept_name }.

journal.window.facets.curation.suggestions.receipt.accepted.partial = accettato il suggerimento di { $agent_name } per questa attività. non è stato possibile aggiornare alcune altre attività.

journal.window.facets.curation.suggestions.receipt.accepted.rename = accettato il suggerimento di { $agent_name }. { $former_name } ora è { $new_name }.

journal.window.facets.curation.suggestions.receipt.accepted.simple = accettato il suggerimento di { $agent_name }.

journal.window.facets.curation.suggestions.receipt.dismissed.agent = ignorato. se { $agent_name } lo suggerisce di nuovo, a { $agent_name } viene detto che hai già detto di no.

journal.window.facets.curation.suggestions.receipt.undone.alias = annullato. { $new_name } non è più un altro nome per { $former_name }.

journal.window.facets.curation.suggestions.receipt.undone.rename = annullato. { $new_name } è di nuovo { $former_name }.

journal.window.facets.curation.suggestions.retry = riprova

journal.window.facets.curation.suggestions.show.all = mostra ogni agente

journal.window.facets.curation.suggestions.show.more = mostra altro

journal.window.facets.curation.suggestions.stale.changed.after.agent = questo è cambiato dopo che { $agent_name } l'ha suggerito, quindi non c'è niente da accettare. ecco com'è ora:

journal.window.facets.curation.suggestions.stale.chip = già cambiato

journal.window.facets.curation.suggestions.stale.pair.changed.after.agent = { $first_name } o { $second_name } ha avuto un cambiamento dopo che { $agent_name } ha suggerito questo, quindi non c'è niente da accettare.

journal.window.facets.curation.suggestions.stale.pair.journal.candidate = il tuo diario sta già chiedendo se { $first_name } e { $second_name } sono la stessa persona. puoi rispondere nei possibili duplicati.

journal.window.facets.curation.suggestions.stale.pair.journal.question = il tuo diario ha una domanda sua su { $first_name } e { $second_name }, quindi qui non c'è niente da accettare.

journal.window.facets.curation.suggestions.stale.pair.owner.answered = hai già tenuto separati { $first_name } e { $second_name }, quindi non c'è niente da accettare.

journal.window.facets.curation.suggestions.suggested = suggerito

journal.window.facets.curation.suggestions.then = se accetti

journal.window.facets.curation.suggestions.then.account.days = il tuo diario ragiona di nuovo su questa attività con il tuo racconto, e su { $affected_days }.

journal.window.facets.curation.suggestions.then.activities.days = { $quantity ->
    [one] il tuo diario ragiona di nuovo su questa attività, e su { $affected_days }.
    [many] il tuo diario ragiona di nuovo su queste { $count } di attività, e su { $affected_days }.
   *[other] il tuo diario ragiona di nuovo su queste { $count } attività, e su { $affected_days }.
    }

journal.window.facets.curation.suggestions.then.alias.name = il tuo diario chiama questa persona anche { $new_name }. la versione precedente resta nella cronologia delle versioni, dove puoi ripristinarla.

journal.window.facets.curation.suggestions.then.merge.people = il tuo diario tiene una persona chiamata { $kept_name }, chiamata anche { $merged_name }, e sposta su di lei tutto ciò che è associato a { $merged_name }. un'unione non si può annullare.

journal.window.facets.curation.suggestions.then.retain.previous.name = il tuo diario chiama questa persona { $new_name } d'ora in poi. { $former_name } resta nella cronologia delle versioni, dove puoi ripristinarlo.

journal.window.facets.curation.suggestions.undo = annulla

journal.window.facets.curation.suggestions.undone.plain = annullato.

journal.window.facets.curation.suggestions.updating = aggiornamento in corso…

journal.window.facets.curation.suggestions.why = perché

journal.window.facets.curation.text.loading.suggestions = caricamento dei suggerimenti…

journal.window.facets.curation.visible.text.cluster = raggruppamento:

journal.window.facets.curation.visible.text.could.not.load.suggestions = non è stato possibile caricare i suggerimenti.

journal.window.facets.curation.visible.text.day = giorno:

journal.window.facets.curation.visible.text.loading.suggestions = caricamento dei suggerimenti…

journal.window.facets.curation.visible.text.segment = segmento:

journal.window.facets.curation.visible.text.source = fonte:

journal.window.facets.curation.visible.text.stream = flusso:

journal.window.facets.curation.window.explanation.a.close.spelling.match.was.chosen = una corrispondenza con grafia simile è stata scelta automaticamente

journal.window.facets.curation.window.explanation.couldn.t.accept.that.so.nothing = non è stato possibile accettarlo, quindi non è cambiato niente. riprova tra un momento.

journal.window.facets.curation.window.explanation.couldn.t.load.what.your.agents = non è stato possibile caricare ciò che i tuoi agenti hanno suggerito.

journal.window.facets.curation.window.explanation.couldn.t.undo.that.because.you = non è stato possibile annullarlo, perché l'hai cambiato dopo. la tua modifica resta.

journal.window.facets.curation.window.explanation.earlier.from.your.agents = prima dai tuoi agenti

journal.window.facets.curation.window.explanation.merge.them.into.one.person = uniscili in una persona sola

journal.window.facets.curation.window.explanation.possible.duplicate.speaker.names = possibili nomi di parlante duplicati

journal.window.facets.curation.window.explanation.saved.for.some.activities = salvato per alcune attività

journal.window.facets.curation.window.explanation.that.didn.t.go.through.try = non è andato a buon fine. riprova.

journal.window.facets.curation.window.explanation.the.names.have.little.in.common = i nomi hanno poco in comune

journal.window.facets.curation.window.explanation.the.suggested.entity.is.no.longer = l'entità suggerita non è più qui

journal.window.facets.curation.window.explanation.the.suggested.entity.is.not.a = l'entità suggerita non è una persona

journal.window.facets.curation.window.explanation.this.can.t.be.accepted.right = questo non si può accettare ora.

journal.window.facets.curation.window.explanation.this.was.a.recording.or.video = era una registrazione o un video in riproduzione

journal.window.facets.curation.window.explanation.undone.but.your.journal.couldn.t = annullato, ma il tuo diario non ha potuto confermare che ci ragionerà di nuovo.

journal.window.facets.curation.window.explanation.undone.your.journal.will.think.about = annullato. il tuo diario ci ragionerà di nuovo senza quella modifica.

journal.window.facets.curation.window.explanation.what.you.accept.is.kept.as = ciò che accetti è tenuto come il tuo racconto.

journal.window.facets.curation.window.explanation.you.accepted.it.then.undid.it = l'hai accettato, poi l'hai annullato

journal.window.facets.curation.window.explanation.you.kept.them.separate = li hai tenuti separati

journal.window.facets.curation.window.explanation.your.account.of.it = il tuo racconto

journal.window.facets.curation.window.explanation.your.journal.didn.t.answer.so = il tuo diario non ha risposto, quindi questo può essere già fatto. riprovare non lo fa due volte.

journal.window.facets.curation.window.explanation.your.journal.didn.t.answer.so.variant2 = il tuo diario non ha risposto, quindi questo può essere già accettato. riprovare non lo accetta due volte.

journal.window.facets.curation.window.explanation.your.journal.will.think.this.through = il tuo diario ci ragionerà di nuovo, come qualcosa che hai guardato o ascoltato, senza nessun altro che partecipi.

journal.window.location.day.area.unnamed = senza una località con nome nelle vicinanze ({ $area })

journal.window.location.day.empty = la posizione è stata conservata, ma non è stato possibile dare un nome a nessuna lettura.

journal.window.location.day.heading = posizione del { $date }

journal.window.location.day.instant = alle { $time }

journal.window.location.day.moment = { $date } alle { $clock }

journal.window.location.day.nearby.list = vicino a { $leading } e a { $last }

journal.window.location.day.place.context = { $specific }; { $broader }

journal.window.location.day.place.inside = a { $place } in { $inside }

journal.window.location.day.place.named = a { $owner_name } (un luogo che hai chiamato); { $location }

journal.window.location.day.readings.area = { $quantity ->
    [one] letture { $area }, { $when }: { $count } lettura da { $device }.
    [many] letture { $area }, { $when }: { $count } di letture da { $device }.
   *[other] letture { $area }, { $when }: { $count } letture da { $device }.
    }

journal.window.location.day.readings.area.approximate = { $quantity ->
    [one] letture { $area }, { $when }: { $count } lettura approssimativa da { $device }.
    [many] letture { $area }, { $when }: { $count } di letture approssimative da { $device }.
   *[other] letture { $area }, { $when }: { $count } letture approssimative da { $device }.
    }

journal.window.location.day.readings.unlocated = { $quantity ->
    [one] letture, { $when }: { $count } lettura da { $device }.
    [many] letture, { $when }: { $count } di letture da { $device }.
   *[other] letture, { $when }: { $count } letture da { $device }.
    }

journal.window.location.day.readings.unlocated.approximate = { $quantity ->
    [one] letture, { $when }: { $count } lettura approssimativa da { $device }.
    [many] letture, { $when }: { $count } di letture approssimative da { $device }.
   *[other] letture, { $when }: { $count } letture approssimative da { $device }.
    }

journal.window.location.day.stay = una permanenza che il tuo diario ha ricavato da { $count } letture da { $device } { $location }: prima lettura { $first }, ultima { $last }.

journal.window.location.day.visit = una visita che { $device } ha segnalato { $location }: { $arrival }, { $departure }.

journal.window.map.accessible.name.edit.this.place = modifica questo luogo

journal.window.map.accessible.name.location.accuracy = precisione della posizione

journal.window.map.accessible.name.map = mappa

journal.window.map.accessible.name.map.of.the.records.listed.here = mappa dei registri elencati qui, con località e confini

journal.window.map.accessible.name.more.of.your.location = altro della tua posizione

journal.window.map.accessible.name.other.weeks.and.this.month = altre settimane e questo mese

journal.window.map.accessible.name.the.day.in.order = il giorno, in ordine

journal.window.map.accessible.name.the.stops.across.the.day.shown = le fermate nell'arco del giorno, mostrate nell'elenco sotto

journal.window.map.accessible.name.visits.and.stays.across.the.day = visite e permanenze nell'arco del giorno

journal.window.map.accessible.name.zoom = zoom

journal.window.map.accessible.name.zoom.in = ingrandisci

journal.window.map.accessible.name.zoom.out = riduci

journal.window.map.accuracy.download.button = scarica { $size }

journal.window.map.accuracy.download.explanation = è{ $slot1 }{ $size }{ $slot2 }da { $slot3 }, lo stesso scaricamento per tutti, quindi ottenerlo non dice niente su dove ti trovavi. i nomi si cercano sul tuo computer.

journal.window.map.accuracy.download.progress = scaricamento della precisione della posizione:{ $slot1 }{ $received }{ $slot2 }di{ $slot3 }{ $total }. puoi continuare a usare il tuo diario mentre si scarica.

journal.window.map.accuracy.explanation.names = una visita che il tuo telefono segnala prende il nome dai luoghi entro il margine di errore del tuo telefono: "vicino a { $slot1 }".

journal.window.map.accuracy.explanation.outlines = una visita che il tuo telefono segnala prende il nome dall'edificio, dal parco o dal campus in cui si trovava, e dai luoghi entro il margine di errore del tuo telefono: "a { $slot1 }", "vicino a { $slot2 }".

journal.window.map.accuracy.offered.names = con la precisione della posizione, una visita che il tuo telefono segnala prende il nome dai luoghi entro il margine di errore del tuo telefono: "vicino a { $slot1 }".

journal.window.map.accuracy.offered.outlines = con la precisione della posizione, una visita che il tuo telefono segnala prende il nome dall'edificio, dal parco o dal campus in cui si trovava, e dai luoghi entro il margine di errore del tuo telefono: "a { $slot1 }", "vicino a { $slot2 }".

journal.window.map.accuracy.remove.effect = libera { $storage }; i nomi tornano alla località vicina.

journal.window.map.accuracy.status.verification.unavailable = non è stato possibile verificare i pacchetti di precisione installati

journal.window.map.accuracy.storage.insufficient = non c'è abbastanza spazio libero: la precisione della posizione richiede { $required_storage } liberi, più un po' di spazio in più.

journal.window.map.accuracy.update.available = la precisione della posizione è attiva. un download più recente è pronto da updates.solstone.app: { $size }.

journal.window.map.area.label.at.place = a{ $slot1 }{ $name }

journal.window.map.area.label.inside = in{ $slot1 }{ $place }

journal.window.map.area.label.inside.qualified = in{ $slot1 }{ $place }{ $slot2 }{ $qualifier }

journal.window.map.area.label.near = vicino a{ $slot1 }{ $place }{ $slot2 }{ $qualifier }

journal.window.map.area.label.near.place = vicino a{ $slot1 }{ $name }

journal.window.map.area.label.qualified = { $region }{ $slot1 }{ $country }

journal.window.map.area.no.town = un luogo senza una località con nome nelle vicinanze

journal.window.map.area.size = circa{ $slot1 }{ $size }{ $slot2 }km

journal.window.map.attribution.base = località da { $geonames_start }GeoNames{ $geonames_end } ({ $licence_start }CC BY 4.0{ $licence_end }) · confini da Natural Earth

journal.window.map.attribution.outlines = località da { $geonames_start }GeoNames{ $geonames_end } ({ $licence_start }CC BY 4.0{ $licence_end }) · confini da Natural Earth · luoghi e contorni da { $overture_start }Overture Maps{ $overture_end }

journal.window.map.attribution.places = località da { $geonames_start }GeoNames{ $geonames_end } ({ $licence_start }CC BY 4.0{ $licence_end }) · confini da Natural Earth · luoghi da { $overture_start }Overture Maps{ $overture_end }

journal.window.map.button.label.clear = svuota

journal.window.map.button.label.download.again = scarica di nuovo

journal.window.map.button.label.fit = adatta

journal.window.map.button.label.remove.location.accuracy = rimuovi la precisione della posizione

journal.window.map.button.label.save = salva

journal.window.map.button.label.show.on.the.map = mostra sulla mappa

journal.window.map.button.label.update = aggiorna

journal.window.map.calendar.day.name = { $state ->
    [stops] { $date }, fermate
    [elsewhere] { $date }, fermate altrove: { $places }
    [no-stops] { $date }, nessuna fermata
    [no-location] { $date }, nessuna posizione conservata
    [unreadable] { $date }, non è stato possibile leggere la posizione
    [kept] { $date }, posizione conservata
   *[unknown] { $date }, { $state_label }
    }

journal.window.map.calendar.day.name.selected = { $state ->
    [stops] { $date }, fermate, ha una fermata nell'area scelta
    [elsewhere] { $date }, fermate altrove: { $places }, ha una fermata nell'area scelta
    [no-stops] { $date }, nessuna fermata, ha una fermata nell'area scelta
    [no-location] { $date }, nessuna posizione conservata, ha una fermata nell'area scelta
    [unreadable] { $date }, non è stato possibile leggere la posizione, ha una fermata nell'area scelta
    [kept] { $date }, posizione conservata, ha una fermata nell'area scelta
   *[unknown] { $date }, { $state_label }, ha una fermata nell'area scelta
    }

journal.window.map.calendar.group.accessible.name = giorni di{ $slot1 }{ $month }. usa i tasti freccia per spostarti tra i giorni.

journal.window.map.calendar.location.kept = posizione conservata

journal.window.map.calendar.location.none = nessuna posizione conservata

journal.window.map.calendar.stops.elsewhere = fermate altrove

journal.window.map.calendar.stops.here = fermate qui

journal.window.map.calendar.stops.none = nessuna fermata

journal.window.map.count.area = { $quantity ->
    [one] { $count } area
    [many] { $count } di aree
   *[other] { $count } aree
    }

journal.window.map.count.day = { $quantity ->
    [one] { $count } giorno
    [many] { $count } di giorni
   *[other] { $count } giorni
    }

journal.window.map.count.night = { $quantity ->
    [one] { $count } notte
    [many] { $count } di notti
   *[other] { $count } notti
    }

journal.window.map.count.reading = { $quantity ->
    [one] { $count } lettura
    [many] { $count } di letture
   *[other] { $count } letture
    }

journal.window.map.count.record = { $quantity ->
    [one] { $count } registro
    [many] { $count } di registri
   *[other] { $count } registri
    }

journal.window.map.count.visit = { $quantity ->
    [one] { $count } visita
    [many] { $count } di visite
   *[other] { $count } visite
    }

journal.window.map.date.long = { $weekday },{ $slot1 }{ $day }{ $slot2 }{ $month }

journal.window.map.date.short = { $month }{ $slot1 }{ $day }

journal.window.map.date.short.weekday = { $weekday }{ $slot1 }{ $date }

journal.window.map.date.short.weekday.comma = { $weekday },{ $slot1 }{ $date }

journal.window.map.date.time = { $day }{ $slot1 }{ $clock }

journal.window.map.date.time.range = { $first }{ $slot1 }a{ $slot2 }{ $last }

journal.window.map.day.location.none = il tuo diario non ha una posizione del { $date }. la posizione può essere stata in pausa o disattivata, oppure il telefono può non essersi connesso al tuo diario. il tuo diario non può dire quale.

journal.window.map.device.android.phone = { $slot1 } telefono

journal.window.map.device.android.tablet = { $slot1 } tablet

journal.window.map.device.detail.tier.balanced = livello di dettaglio nell'app di solstone: { $level_start }luoghi + andirivieni{ $level_end }.

journal.window.map.device.detail.tier.full = livello di dettaglio nell'app di solstone: { $level_start }il quadro completo{ $level_end }.

journal.window.map.device.detail.tier.light = livello di dettaglio nell'app di solstone: { $level_start }solo luoghi{ $level_end }.

journal.window.map.device.from = da{ $slot1 }{ $device }

journal.window.map.device.from.android.phone = dal tuo telefono android

journal.window.map.device.from.android.phone.named = dal tuo telefono android ({ $device_name })

journal.window.map.device.from.android.tablet = dal tuo tablet android

journal.window.map.device.from.android.tablet.named = dal tuo tablet android ({ $device_name })

journal.window.map.device.from.ipad = dal tuo ipad

journal.window.map.device.from.ipad.named = dal tuo ipad ({ $device_name })

journal.window.map.device.from.iphone = dal tuo iphone

journal.window.map.device.from.iphone.named = dal tuo iphone ({ $device_name })

journal.window.map.device.from.phone = dal tuo telefono

journal.window.map.device.from.phone.named = dal tuo telefono ({ $device_name })

journal.window.map.device.from.tablet = dal tuo tablet

journal.window.map.device.from.tablet.named = dal tuo tablet ({ $device_name })

journal.window.map.device.from.watch = dal tuo orologio

journal.window.map.device.from.watch.named = dal tuo orologio ({ $device_name })

journal.window.map.device.interrupted.android.phone = { $quantity ->
    [one] il tuo telefono android ha detto che la posizione è stata interrotta una volta in questo giorno.
    [many] il tuo telefono android ha detto che la posizione è stata interrotta { $count } di volte in questo giorno.
   *[other] il tuo telefono android ha detto che la posizione è stata interrotta { $count } volte in questo giorno.
    }

journal.window.map.device.interrupted.android.phone.twice = il tuo telefono android ha detto che la posizione è stata interrotta due volte in questo giorno.

journal.window.map.device.interrupted.android.tablet = { $quantity ->
    [one] il tuo tablet android ha detto che la posizione è stata interrotta una volta in questo giorno.
    [many] il tuo tablet android ha detto che la posizione è stata interrotta { $count } di volte in questo giorno.
   *[other] il tuo tablet android ha detto che la posizione è stata interrotta { $count } volte in questo giorno.
    }

journal.window.map.device.interrupted.android.tablet.twice = il tuo tablet android ha detto che la posizione è stata interrotta due volte in questo giorno.

journal.window.map.device.interrupted.ipad = { $quantity ->
    [one] il tuo ipad ha detto che la posizione è stata interrotta una volta in questo giorno.
    [many] il tuo ipad ha detto che la posizione è stata interrotta { $count } di volte in questo giorno.
   *[other] il tuo ipad ha detto che la posizione è stata interrotta { $count } volte in questo giorno.
    }

journal.window.map.device.interrupted.ipad.twice = il tuo ipad ha detto che la posizione è stata interrotta due volte in questo giorno.

journal.window.map.device.interrupted.iphone = { $quantity ->
    [one] il tuo iphone ha detto che la posizione è stata interrotta una volta in questo giorno.
    [many] il tuo iphone ha detto che la posizione è stata interrotta { $count } di volte in questo giorno.
   *[other] il tuo iphone ha detto che la posizione è stata interrotta { $count } volte in questo giorno.
    }

journal.window.map.device.interrupted.iphone.twice = il tuo iphone ha detto che la posizione è stata interrotta due volte in questo giorno.

journal.window.map.device.interrupted.phone = { $quantity ->
    [one] il tuo telefono ha detto che la posizione è stata interrotta una volta in questo giorno.
    [many] il tuo telefono ha detto che la posizione è stata interrotta { $count } di volte in questo giorno.
   *[other] il tuo telefono ha detto che la posizione è stata interrotta { $count } volte in questo giorno.
    }

journal.window.map.device.interrupted.phone.twice = il tuo telefono ha detto che la posizione è stata interrotta due volte in questo giorno.

journal.window.map.device.interrupted.tablet = { $quantity ->
    [one] il tuo tablet ha detto che la posizione è stata interrotta una volta in questo giorno.
    [many] il tuo tablet ha detto che la posizione è stata interrotta { $count } di volte in questo giorno.
   *[other] il tuo tablet ha detto che la posizione è stata interrotta { $count } volte in questo giorno.
    }

journal.window.map.device.interrupted.tablet.twice = il tuo tablet ha detto che la posizione è stata interrotta due volte in questo giorno.

journal.window.map.device.interrupted.unknown = { $quantity ->
    [one] il tuo { $device } ha detto che la posizione è stata interrotta una volta in questo giorno.
    [many] il tuo { $device } ha detto che la posizione è stata interrotta { $count } di volte in questo giorno.
   *[other] il tuo { $device } ha detto che la posizione è stata interrotta { $count } volte in questo giorno.
    }

journal.window.map.device.interrupted.unknown.twice = il tuo { $device } ha detto che la posizione è stata interrotta due volte in questo giorno.

journal.window.map.device.interrupted.watch = { $quantity ->
    [one] il tuo orologio ha detto che la posizione è stata interrotta una volta in questo giorno.
    [many] il tuo orologio ha detto che la posizione è stata interrotta { $count } di volte in questo giorno.
   *[other] il tuo orologio ha detto che la posizione è stata interrotta { $count } volte in questo giorno.
    }

journal.window.map.device.interrupted.watch.twice = il tuo orologio ha detto che la posizione è stata interrotta due volte in questo giorno.

journal.window.map.device.phone = telefono

journal.window.map.device.possessive = il tuo{ $slot1 }{ $device }

journal.window.map.device.possessive.named = il tuo{ $slot1 }{ $device }{ $slot2 }({ $name })

journal.window.map.device.precision.off = la posizione precisa è disattivata per solstone sul tuo { $device }, quindi le sue letture indicano la località, non l'edificio.

journal.window.map.device.precision.off.android.phone = la posizione precisa è disattivata per solstone sul tuo telefono android, quindi le sue letture indicano la località, non l'edificio.

journal.window.map.device.precision.off.android.tablet = la posizione precisa è disattivata per solstone sul tuo tablet android, quindi le sue letture indicano la località, non l'edificio.

journal.window.map.device.precision.off.ipad = la posizione precisa è disattivata per solstone sul tuo ipad, quindi le sue letture indicano la località, non l'edificio.

journal.window.map.device.precision.off.iphone = la posizione precisa è disattivata per solstone sul tuo iphone, quindi le sue letture indicano la località, non l'edificio.

journal.window.map.device.precision.off.phone = la posizione precisa è disattivata per solstone sul tuo telefono, quindi le sue letture indicano la località, non l'edificio.

journal.window.map.device.precision.off.tablet = la posizione precisa è disattivata per solstone sul tuo tablet, quindi le sue letture indicano la località, non l'edificio.

journal.window.map.device.precision.off.watch = la posizione precisa è disattivata per solstone sul tuo orologio, quindi le sue letture indicano la località, non l'edificio.

journal.window.map.device.stay.android.phone = una permanenza che il tuo diario ha ricavato da letture dal tuo telefono android

journal.window.map.device.stay.android.phone.named = una permanenza che il tuo diario ha ricavato da letture dal tuo telefono android ({ $device_name })

journal.window.map.device.stay.android.tablet = una permanenza che il tuo diario ha ricavato dalle letture del tuo tablet android

journal.window.map.device.stay.android.tablet.named = una permanenza che il tuo diario ha ricavato dalle letture del tuo tablet android ({ $device_name })

journal.window.map.device.stay.ipad = una permanenza che il tuo diario ha ricavato dalle letture del tuo ipad

journal.window.map.device.stay.ipad.named = una permanenza che il tuo diario ha ricavato dalle letture del tuo ipad ({ $device_name })

journal.window.map.device.stay.iphone = una permanenza che il tuo diario ha ricavato dalle letture del tuo iphone

journal.window.map.device.stay.iphone.named = una permanenza che il tuo diario ha ricavato dalle letture del tuo iphone ({ $device_name })

journal.window.map.device.stay.phone = una permanenza che il tuo diario ha ricavato dalle letture del tuo telefono

journal.window.map.device.stay.phone.named = una permanenza che il tuo diario ha ricavato dalle letture del tuo telefono ({ $device_name })

journal.window.map.device.stay.tablet = una permanenza che il tuo diario ha ricavato dalle letture del tuo tablet

journal.window.map.device.stay.tablet.named = una permanenza che il tuo diario ha ricavato dalle letture del tuo tablet ({ $device_name })

journal.window.map.device.stay.watch = una permanenza che il tuo diario ha ricavato dalle letture del tuo orologio

journal.window.map.device.stay.watch.named = una permanenza che il tuo diario ha ricavato dalle letture del tuo orologio ({ $device_name })

journal.window.map.device.tablet = tablet

journal.window.map.device.visit.android.phone = una visita segnalata dal tuo telefono android

journal.window.map.device.visit.android.phone.named = una visita segnalata dal tuo telefono android ({ $device_name })

journal.window.map.device.visit.android.tablet = una visita segnalata dal tuo tablet android

journal.window.map.device.visit.android.tablet.named = una visita segnalata dal tuo tablet android ({ $device_name })

journal.window.map.device.visit.ipad = una visita segnalata dal tuo ipad

journal.window.map.device.visit.ipad.named = una visita segnalata dal tuo ipad ({ $device_name })

journal.window.map.device.visit.iphone = una visita segnalata dal tuo iphone

journal.window.map.device.visit.iphone.named = una visita segnalata dal tuo iphone ({ $device_name })

journal.window.map.device.visit.phone = una visita segnalata dal tuo telefono

journal.window.map.device.visit.phone.named = una visita segnalata dal tuo telefono ({ $device_name })

journal.window.map.device.visit.tablet = una visita segnalata dal tuo tablet

journal.window.map.device.visit.tablet.named = una visita segnalata dal tuo tablet ({ $device_name })

journal.window.map.device.visit.watch = una visita segnalata dal tuo orologio

journal.window.map.device.visit.watch.named = una visita segnalata dal tuo orologio ({ $device_name })

journal.window.map.disclosure.label.how.places.are.worked.out = come si ricavano i luoghi

journal.window.map.disclosure.label.how.the.month.is.drawn = come si disegna il mese

journal.window.map.disclosure.label.how.this.is.drawn = come si disegna questo

journal.window.map.disclosure.label.licences = licenze

journal.window.map.disclosure.label.why.these.words = perché queste parole

journal.window.map.entry.area.town.reason = { $town } è la località che meglio rappresenta quest'area, a { $distance_km } km di distanza.

journal.window.map.entry.lines.unreadable = { $quantity ->
    [one] { $count } riga che non è stato possibile leggere
    [many] { $count } di righe che non è stato possibile leggere
   *[other] { $count } righe che non è stato possibile leggere
    }

journal.window.map.entry.margin.within = tutta quell'area è dentro { $area }.

journal.window.map.entry.open.arrival = fino a { $departure }, arrivo non noto

journal.window.map.entry.open.departure = da { $arrival }, nessuna partenza segnalata

journal.window.map.entry.precision.approximate = approssimativa

journal.window.map.entry.readings.within = ogni lettura qui, con il suo margine di errore, è dentro { $area }.

journal.window.map.entry.reported.visit = una visita{ $slot1 }{ $device }{ $slot2 }segnalata

journal.window.map.entry.stay.margin.inside.place = queste letture, con i loro margini, rientrano tutte entro { $margin_meters } m dal loro centro, e tutto questo è dentro { $place }.

journal.window.map.entry.stay.margin.near.place = { $place } è a { $distance_meters } m; queste letture, con i loro margini, rientrano tutte entro { $margin_meters } m dal loro centro.

journal.window.map.entry.stay.margin.near.place.category = { $place } ({ $category }) è a { $distance_meters } m; queste letture, con i loro margini, rientrano tutte entro { $margin_meters } m dal loro centro.

journal.window.map.entry.stay.precise.reason = { $quantity ->
    [one] { $count } lettura precisa è elencata per questa permanenza.
    [many] il tuo diario ha ricavato questa permanenza da { $count } di letture precise che sono rimaste entro 100 m dalla prima di esse per almeno cinque minuti. i suoi orari sono la prima e l'ultima di esse.
   *[other] il tuo diario ha ricavato questa permanenza da { $count } letture precise che sono rimaste entro 100 m dalla prima di esse per almeno cinque minuti. i suoi orari sono la prima e l'ultima di esse.
    }

journal.window.map.entry.stay.recorded.device = una permanenza che il tuo diario ha ricavato dalle letture di { $device }

journal.window.map.entry.stay.within = queste letture, con i loro margini, rientrano tutte dentro { $area }.

journal.window.map.entry.time.arrival = arrivato{ $slot1 }{ $time }

journal.window.map.entry.time.departure = partito{ $slot1 }{ $time }

journal.window.map.entry.time.missing.arrival = arrivo non noto

journal.window.map.entry.time.missing.departure = nessuna partenza segnalata

journal.window.map.entry.transcripts.segments = { $quantity ->
    [one] apri nelle trascrizioni: { $count } segmento
    [many] apri nelle trascrizioni: { $count } di segmenti
   *[other] apri nelle trascrizioni: { $count } segmenti
    }

journal.window.map.entry.visit.margin.inside.place = il tuo telefono ha indicato la tua posizione entro { $margin_meters } m, e tutto questo è dentro { $place }.

journal.window.map.entry.visit.margin.near.place = { $place } è a { $distance_meters } m; il tuo telefono ha indicato la tua posizione entro { $margin_meters } m.

journal.window.map.entry.visit.margin.near.place.category = { $place } ({ $category }) è a { $distance_meters } m; il tuo telefono ha indicato la tua posizione entro { $margin_meters } m.

journal.window.map.entry.visit.margin.too.wide = il tuo telefono ha indicato la tua posizione entro { $margin_meters } m, troppo ampia per nominare un edificio o i luoghi vicini.

journal.window.map.entry.visit.within = la posizione del tuo telefono, con il suo margine di errore, è dentro { $area }.

journal.window.map.field.label.call.it = chiamalo

journal.window.map.field.label.hide.this.place = nascondi questo luogo

journal.window.map.field.label.name = nome

journal.window.map.heading.couldn.t.open.this.day.s = non è stato possibile aprire la posizione di questo giorno.

journal.window.map.heading.no.location.in.your.journal.yet = ancora nessuna posizione nel tuo diario

journal.window.map.heading.no.location.kept.for.this.day = nessuna posizione conservata per questo giorno

journal.window.map.label.about.km = circa 1 km

journal.window.map.leaflet.zoom.in = Ingrandisci

journal.window.map.leaflet.zoom.out = Riduci

journal.window.map.link.label.next.month = mese successivo

journal.window.map.link.label.open.in.transcripts = apri nelle trascrizioni

journal.window.map.link.label.open.the.day = apri il giorno

journal.window.map.link.label.place = luogo

journal.window.map.link.label.places = luoghi

journal.window.map.link.label.previous.month = mese precedente

journal.window.map.link.label.the.day = il giorno

journal.window.map.link.label.the.week.after = la settimana dopo

journal.window.map.link.label.the.week.before = la settimana prima

journal.window.map.link.label.this.month = questo mese

journal.window.map.link.label.this.week = questa settimana

journal.window.map.list.conjunction = { $leading }{ $slot1 }e{ $slot2 }{ $last }

journal.window.map.month.area.around = intorno a{ $slot1 }{ $name }

journal.window.map.month.area.clipped = solo la parte in { $month } conta.

journal.window.map.month.area.days.in.month = { $quantity ->
    [one] { $count } giorno questo mese
    [many] { $count } di giorni questo mese
   *[other] { $count } giorni questo mese
    }

journal.window.map.month.area.days.ratio = { $days_with_stops } di { $days_in_month }

journal.window.map.month.area.days.show.all = { $quantity ->
    [one] mostra { $count } giorno
    [many] mostra tutti i { $count } di giorni
   *[other] mostra tutti i { $count } giorni
    }

journal.window.map.month.area.distance.first = { $direction ->
    *[east] { $distance } km a est della prima area elencata
    [north-east] { $distance } km a nord-est della prima area elencata
    [north] { $distance } km a nord della prima area elencata
    [north-west] { $distance } km a nord-ovest della prima area elencata
    [west] { $distance } km a ovest della prima area elencata
    [south-west] { $distance } km a sud-ovest della prima area elencata
    [south] { $distance } km a sud della prima area elencata
    [south-east] { $distance } km a sud-est della prima area elencata
    }

journal.window.map.month.area.distance.month = { $direction ->
    *[east] { $distance } km a est dell'area più frequentata
    [north-east] { $distance } km a nord-est dell'area più frequentata
    [north] { $distance } km a nord dell'area più frequentata
    [north-west] { $distance } km a nord-ovest dell'area più frequentata
    [west] { $distance } km a ovest dell'area più frequentata
    [south-west] { $distance } km a sud-ovest dell'area più frequentata
    [south] { $distance } km a sud dell'area più frequentata
    [south-east] { $distance } km a sud-est dell'area più frequentata
    }

journal.window.map.month.area.distance.trip = { $direction ->
    *[east] { $distance } km a est dell'area più frequentata di questo viaggio
    [north-east] { $distance } km a nord-est dell'area più frequentata di questo viaggio
    [north] { $distance } km a nord dell'area più frequentata di questo viaggio
    [north-west] { $distance } km a nord-ovest dell'area più frequentata di questo viaggio
    [west] { $distance } km a ovest dell'area più frequentata di questo viaggio
    [south-west] { $distance } km a sud-ovest dell'area più frequentata di questo viaggio
    [south] { $distance } km a sud dell'area più frequentata di questo viaggio
    [south-east] { $distance } km a sud-est dell'area più frequentata di questo viaggio
    }

journal.window.map.month.area.live.days = { $quantity ->
    [one] { $name }, { $count } giorno questo mese
    [many] { $name }, { $count } di giorni questo mese
   *[other] { $name }, { $count } giorni questo mese
    }

journal.window.map.month.area.open.stops = { $quantity ->
    [one] 1 visita non ha il suo arrivo o la sua partenza, quindi il suo tempo non è nelle ore o nelle notti. il suo giorno conta comunque.
    [many] { $count } di visite non hanno un arrivo o una partenza, quindi il loro tempo non è nelle ore o nelle notti. i loro giorni contano comunque.
   *[other] { $count } visite non hanno un arrivo o una partenza, quindi il loro tempo non è nelle ore o nelle notti. i loro giorni contano comunque.
    }

journal.window.map.month.area.size = un'area { $size } di lato

journal.window.map.month.areas.list.region = { $quantity ->
    [one] { $count } area { $area }, prima quelle con più giorni.
    [many] { $count } di aree { $area }, prima quelle con più giorni.
   *[other] { $count } aree { $area }, prima quelle con più giorni.
    }

journal.window.map.month.areas.list.region.home = { $quantity ->
    [one] { $count } area intorno a { $area }, prima quelle con più giorni.
    [many] { $count } di aree intorno a { $area }, prima quelle con più giorni.
   *[other] { $count } aree intorno a { $area }, prima quelle con più giorni.
    }

journal.window.map.month.areas.list.region.shared = { $quantity ->
    [one] { $count } area { $area }, tutta in { $region }, prima quelle con più giorni.
    [many] { $count } di aree { $area }, tutte in { $region }, prima quelle con più giorni.
   *[other] { $count } aree { $area }, tutte in { $region }, prima quelle con più giorni.
    }

journal.window.map.month.areas.list.region.shared.home = { $quantity ->
    [one] { $count } area intorno a { $area }, tutta in { $region }, prima quelle con più giorni.
    [many] { $count } di aree intorno a { $area }, tutte in { $region }, prima quelle con più giorni.
   *[other] { $count } aree intorno a { $area }, tutte in { $region }, prima quelle con più giorni.
    }

journal.window.map.month.areas.list.trip = { $quantity ->
    [one] { $count } area nel viaggio di { $date }, prima quelle con più giorni.
    [many] { $count } di aree nel viaggio di { $date }, prima quelle con più giorni.
   *[other] { $count } aree nel viaggio di { $date }, prima quelle con più giorni.
    }

journal.window.map.month.areas.list.trip.shared = { $quantity ->
    [one] { $count } area nel viaggio di { $date }, tutta in { $region }, prima quelle con più giorni.
    [many] { $count } di aree nel viaggio di { $date }, tutte in { $region }, prima quelle con più giorni.
   *[other] { $count } aree nel viaggio di { $date }, tutte in { $region }, prima quelle con più giorni.
    }

journal.window.map.month.areas.live.zoom = { $quantity ->
    [one] ingrandito su { $count } area
    [many] ingrandito su { $count } di aree
   *[other] ingrandito su { $count } aree
    }

journal.window.map.month.areas.outlined = { $quantity ->
    [one] { $count } area contornata
    [many] { $count } di aree contornate
   *[other] { $count } aree contornate
    }

journal.window.map.month.areas.outside.show.all = { $quantity ->
    [one] { $count } area fuori dalla mappa · mostra tutto
    [many] { $count } di aree fuori dalla mappa · mostra tutto
   *[other] { $count } aree fuori dalla mappa · mostra tutto
    }

journal.window.map.month.areas.show.all = { $quantity ->
    [one] mostra { $count } area
    [many] mostra tutte le { $count } di aree
   *[other] mostra tutte le { $count } aree
    }

journal.window.map.month.choice.trip = nel viaggio di{ $slot1 }{ $date }{ $slot2 }

journal.window.map.month.day.location.none = il tuo diario non ha una posizione da{ $slot1 }{ $date }. la posizione può essere stata in pausa o disattivata, oppure il telefono può non essersi connesso al tuo diario. il tuo diario non può dire quale.

journal.window.map.month.day.stay.readings = { $quantity ->
    [one] { $count } lettura del tuo telefono è elencata per questa permanenza
    [many] una permanenza che il tuo diario ha ricavato da { $count } di letture del tuo telefono
   *[other] una permanenza che il tuo diario ha ricavato da { $count } letture del tuo telefono
    }

journal.window.map.month.days.show.fewer = mostra meno giorni

journal.window.map.month.explanation.midnight = solo il tempo in{ $slot1 }{ $month }{ $slot2 }conta. una visita iniziata il{ $slot3 }{ $day }{ $slot4 }{ $previous }{ $slot5 }conta da mezzanotte.

journal.window.map.month.heading = { $month }{ $slot1 }{ $year }

journal.window.map.month.live.areas.all = mostrando ogni area

journal.window.map.month.live.choice.none = niente scelto

journal.window.map.month.live.fit = mappa adattata

journal.window.map.month.live.showing.choice = mostrando{ $slot1 }{ $name }

journal.window.map.month.live.showing.home = mostrando intorno a { $area }

journal.window.map.month.live.zoom.amount = ingrandito{ $slot1 }{ $zoom }

journal.window.map.month.location.gaps = { $quantity ->
    [one] nessuna posizione è stata conservata in { $count } giorno.
    [many] nessuna posizione è stata conservata in { $count } di giorni.
   *[other] nessuna posizione è stata conservata in { $count } giorni.
    }

journal.window.map.month.map.area.summary = { $shaded_quantity ->
    [one] mappa di { $area }: { $shaded_areas } area ombreggiata, { $size } di lato. la stessa area è elencata sotto le aree di questo mese.
    [many] mappa di { $area }: { $shaded_areas } di aree ombreggiate, ciascuna di { $size } di lato. le stesse aree sono elencate sotto le aree di questo mese.
   *[other] mappa di { $area }: { $shaded_areas } aree ombreggiate, ciascuna di { $size } di lato. le stesse aree sono elencate sotto le aree di questo mese.
    }

journal.window.map.month.map.area.summary.home = { $shaded_quantity ->
    [one] mappa dell'area intorno a { $area }: { $shaded_areas } area ombreggiata, { $size } di lato. la stessa area è elencata sotto le aree di questo mese.
    [many] mappa dell'area intorno a { $area }: { $shaded_areas } di aree ombreggiate, ciascuna di { $size } di lato. le stesse aree sono elencate sotto le aree di questo mese.
   *[other] mappa dell'area intorno a { $area }: { $shaded_areas } aree ombreggiate, ciascuna di { $size } di lato. le stesse aree sono elencate sotto le aree di questo mese.
    }

journal.window.map.month.map.group.summary = { $shaded_quantity ->
    [one] { $area_quantity ->
        [one] mappa di { $area }: { $shaded_areas } area ombreggiata, { $size } di lato, che contiene { $areas } area elencata sotto le aree di questo mese.
        [many] mappa di { $area }: { $shaded_areas } area ombreggiata, { $size } di lato, che contiene { $areas } di aree elencate sotto le aree di questo mese.
       *[other] mappa di { $area }: { $shaded_areas } area ombreggiata, { $size } di lato, che contiene { $areas } aree elencate sotto le aree di questo mese.
    }
    [many] { $area_quantity ->
        [one] mappa di { $area }: { $shaded_areas } di aree ombreggiate, ciascuna di { $size } di lato, che contiene { $areas } area elencata sotto le aree di questo mese.
        [many] mappa di { $area }: { $shaded_areas } di aree ombreggiate, ciascuna di { $size } di lato, che contiene { $areas } di aree elencate sotto le aree di questo mese.
       *[other] mappa di { $area }: { $shaded_areas } di aree ombreggiate, ciascuna di { $size } di lato, che contiene { $areas } aree elencate sotto le aree di questo mese.
    }
   *[other] { $area_quantity ->
        [one] mappa di { $area }: { $shaded_areas } aree ombreggiate, ciascuna di { $size } di lato, che contiene { $areas } area elencata sotto le aree di questo mese.
        [many] mappa di { $area }: { $shaded_areas } aree ombreggiate, ciascuna di { $size } di lato, che contiene { $areas } di aree elencate sotto le aree di questo mese.
       *[other] mappa di { $area }: { $shaded_areas } aree ombreggiate, ciascuna di { $size } di lato, che contiene { $areas } aree elencate sotto le aree di questo mese.
    }
    }

journal.window.map.month.map.group.summary.home = { $shaded_quantity ->
    [one] { $area_quantity ->
        [one] mappa dell'area intorno a { $area }: { $shaded_areas } area ombreggiata, { $size } di lato, che contiene { $areas } area elencata sotto le aree di questo mese.
        [many] mappa dell'area intorno a { $area }: { $shaded_areas } area ombreggiata, { $size } di lato, che contiene { $areas } di aree elencate sotto le aree di questo mese.
       *[other] mappa dell'area intorno a { $area }: { $shaded_areas } area ombreggiata, { $size } di lato, che contiene { $areas } aree elencate sotto le aree di questo mese.
    }
    [many] { $area_quantity ->
        [one] mappa dell'area intorno a { $area }: { $shaded_areas } di aree ombreggiate, ciascuna di { $size } di lato, che contiene { $areas } area elencata sotto le aree di questo mese.
        [many] mappa dell'area intorno a { $area }: { $shaded_areas } di aree ombreggiate, ciascuna di { $size } di lato, che contiene { $areas } di aree elencate sotto le aree di questo mese.
       *[other] mappa dell'area intorno a { $area }: { $shaded_areas } di aree ombreggiate, ciascuna di { $size } di lato, che contiene { $areas } aree elencate sotto le aree di questo mese.
    }
   *[other] { $area_quantity ->
        [one] mappa dell'area intorno a { $area }: { $shaded_areas } aree ombreggiate, ciascuna di { $size } di lato, che contiene { $areas } area elencata sotto le aree di questo mese.
        [many] mappa dell'area intorno a { $area }: { $shaded_areas } aree ombreggiate, ciascuna di { $size } di lato, che contiene { $areas } di aree elencate sotto le aree di questo mese.
       *[other] mappa dell'area intorno a { $area }: { $shaded_areas } aree ombreggiate, ciascuna di { $size } di lato, che contiene { $areas } aree elencate sotto le aree di questo mese.
    }
    }

journal.window.map.month.map.nothing.drawn = niente disegnato qui

journal.window.map.month.readings.undrawn = { $quantity ->
    [one] non disegnata: { $count } lettura.
    [many] non disegnate: { $count } di letture.
   *[other] non disegnate: { $count } letture.
    }

journal.window.map.month.stops.calendar = { $stops_quantity ->
    [one] { $days_with_stops } di { $days_in_month } giorni ha fermate
    [many] { $days_with_stops } di { $days_in_month } giorni hanno fermate
   *[other] { $days_with_stops } di { $days_in_month } giorni hanno fermate
    }

journal.window.map.month.stops.glance = { $stops_quantity ->
    [one] { $days_with_stops } di { $days_in_month } giorni ha fermate.
    [many] { $days_with_stops } di { $days_in_month } giorni hanno fermate.
   *[other] { $days_with_stops } di { $days_in_month } giorni hanno fermate.
    }

journal.window.map.month.stops.glance.home.days = { $stops_quantity ->
    [one] { $days_with_stops } di { $days_in_month } giorni ha fermate: { $home_days } intorno a { $home_area }.
    [many] { $days_with_stops } di { $days_in_month } giorni hanno fermate: { $home_days } intorno a { $home_area }.
   *[other] { $days_with_stops } di { $days_in_month } giorni hanno fermate: { $home_days } intorno a { $home_area }.
    }

journal.window.map.month.stops.glance.trips.days = { $trip_quantity ->
    [one] { $days_with_stops } di { $days_in_month } giorni { $stops_quantity ->
        [one] ha
        [many] hanno
       *[other] hanno
    } fermate: { $home_days } intorno a { $home_area }, e { $trips } viaggio a più di 50 km.
    [many] { $days_with_stops } di { $days_in_month } giorni { $stops_quantity ->
        [one] ha
        [many] hanno
       *[other] hanno
    } fermate: { $home_days } intorno a { $home_area }, e { $trips } di viaggi a più di 50 km.
   *[other] { $days_with_stops } di { $days_in_month } giorni { $stops_quantity ->
        [one] ha
        [many] hanno
       *[other] hanno
    } fermate: { $home_days } intorno a { $home_area }, e { $trips } viaggi a più di 50 km.
    }

journal.window.map.month.stops.undrawn.hidden = { $quantity ->
    [one] un luogo che hai nascosto
    [many] { $count } di luoghi che hai nascosto
   *[other] { $count } luoghi che hai nascosto
    }

journal.window.map.month.stops.undrawn.wide = { $quantity ->
    [one] { $count } fermata con un margine di errore oltre 500 m o nessuno segnalato
    [many] { $count } di fermate con un margine di errore oltre 500 m o nessuno segnalato
   *[other] { $count } fermate con un margine di errore oltre 500 m o nessuno segnalato
    }

journal.window.map.month.strip.faded.bar = una barra sfumata non ha il suo arrivo o la sua partenza.

journal.window.map.month.strip.visits.and.stays = le barre piene sono visite segnalate dal tuo telefono; le barre contornate sono permanenze che il tuo diario ha ricavato.

journal.window.map.month.time.all.day = tutto il giorno

journal.window.map.month.time.from = da{ $slot1 }{ $time }

journal.window.map.month.time.until = fino a{ $slot1 }{ $time }

journal.window.map.month.zoom.notice = ingrandito.

journal.window.map.months.label.april = Aprile

journal.window.map.months.label.august = Agosto

journal.window.map.months.label.december = Dicembre

journal.window.map.months.label.february = Febbraio

journal.window.map.months.label.january = Gennaio

journal.window.map.months.label.july = Luglio

journal.window.map.months.label.june = Giugno

journal.window.map.months.label.march = Marzo

journal.window.map.months.label.may = Maggio

journal.window.map.months.label.november = Novembre

journal.window.map.months.label.october = Ottobre

journal.window.map.months.label.september = Settembre

journal.window.map.navigation.go.day = vai a { $date }

journal.window.map.navigation.go.month = vai a { $month } { $year }

journal.window.map.option.label.home = casa

journal.window.map.option.label.nothing = niente

journal.window.map.option.label.work = lavoro

journal.window.map.place.named.link = un luogo che hai chiamato "{ $name }"

journal.window.map.place.named.reason = hai chiamato questo luogo "{ $name }".

journal.window.map.place.recorded.dates = { $quantity ->
    [one] visite o permanenze qui in { $count } data.
    [many] visite o permanenze qui in { $count } di date.
   *[other] visite o permanenze qui in { $count } date.
    }

journal.window.map.place.recorded.dates.range = { $quantity ->
    [one] visite o permanenze qui in { $count } data, da { $first } a { $last }.
    [many] visite o permanenze qui in { $count } di date, da { $first } a { $last }.
   *[other] visite o permanenze qui in { $count } date, da { $first } a { $last }.
    }

journal.window.map.place.recorded.dates.single = { $quantity ->
    [one] visite o permanenze qui in { $count } data, { $date }.
    [many] visite o permanenze qui in { $count } di date, { $date }.
   *[other] visite o permanenze qui in { $count } date, { $date }.
    }

journal.window.map.place.recorded.nights = { $quantity ->
    [one] in { $count } notte, una visita o permanenza qui copre almeno due ore tra le 22:00 e le 06:00.
    [many] in { $count } di notti, una visita o permanenza qui copre almeno due ore tra le 22:00 e le 06:00.
   *[other] in { $count } notti, una visita o permanenza qui copre almeno due ore tra le 22:00 e le 06:00.
    }

journal.window.map.place.visibility.hidden = · nascosto

journal.window.map.response.accuracy.busy = la precisione della posizione è già in scaricamento.

journal.window.map.response.accuracy.busy.detail = Uno scaricamento è in corso

journal.window.map.response.accuracy.download.failed = non è stato possibile scaricare la precisione della posizione.

journal.window.map.response.accuracy.not.offered = questa versione del tuo diario non ha uno scaricamento per la precisione della posizione.

journal.window.map.response.accuracy.not.offered.detail = Niente offerto

journal.window.map.response.accuracy.remove.failed = non è stato possibile rimuovere la precisione della posizione.

journal.window.map.response.area.unavailable = non è stato possibile leggere i nomi dei luoghi che vengono con il tuo diario.

journal.window.map.response.day.invalid = non è stato possibile usare quel giorno.

journal.window.map.response.day.invalid.detail = Giorno non trovato

journal.window.map.response.edit.alias = { $slot1 } deve essere { $slot2 }, { $slot3 } o { $slot4 }

journal.window.map.response.edit.body = Body deve essere un oggetto { $slot1 }

journal.window.map.response.edit.failed = non è stato possibile salvare la modifica.

journal.window.map.response.edit.hidden = { $slot1 } deve essere { $slot2 } o { $slot3 }

journal.window.map.response.edit.invalid = non è stato possibile usare quella modifica.

journal.window.map.response.edit.name = { $slot1 } deve essere una stringa o { $slot2 }

journal.window.map.response.month.invalid = non è stato possibile usare quel mese.

journal.window.map.response.month.invalid.detail = Il mese deve essere { $slot1 }

journal.window.map.response.place.unknown = quel luogo non è nel tuo diario.

journal.window.map.response.place.unknown.detail = Luogo non trovato

journal.window.map.section.heading.areas.this.month = aree di questo mese

journal.window.map.section.heading.day.by.day = giorno per giorno

journal.window.map.section.heading.deleting.any.of.this = eliminare una qualsiasi di queste

journal.window.map.section.heading.edit.this.place = modifica questo luogo

journal.window.map.section.heading.location.accuracy = precisione della posizione

journal.window.map.section.heading.the.day.in.order = il giorno, in ordine

journal.window.map.section.heading.the.map = la mappa

journal.window.map.section.heading.the.month.at.a.glance = il mese a colpo d'occhio

journal.window.map.section.heading.visits.and.stays.here = visite e permanenze qui

journal.window.map.status.text.couldn.t.open.this.month.s = non è stato possibile aprire la posizione di questo mese.

journal.window.map.text.loading = caricamento…

journal.window.map.text.loading.this.day.s.location = caricamento della posizione di questo giorno…

journal.window.map.text.loading.your.journal.s.location = caricamento della posizione del tuo diario…

journal.window.map.visible.text.a.hidden.place.is.off.the = un luogo nascosto è fuori dalla mappa, e ciò che il tuo diario scrive su dove eri ogni giorno lo lascia fuori: nella ricerca, per gli agenti connessi e nelle attività. anche i nuovi riepiloghi giornalieri lo lasciano fuori.

journal.window.map.visible.text.a.night.counts.in.an.area = una notte conta in un'area quando una visita lì copre almeno due ore tra le 22:00 e le 06:00.

journal.window.map.visible.text.a.place.is.a.spot.your = un luogo è un punto in cui il telefono ha segnalato una visita, o una permanenza, in due date diverse, abbastanza vicine da essere un solo punto. una posizione approssimativa non diventa mai un luogo.

journal.window.map.visible.text.a.place.lasts.as.long.as = un luogo dura quanto il segmento da cui proviene. elimina quei segmenti nelle trascrizioni e il luogo, e il suo nome, se ne vanno anche loro.

journal.window.map.visible.text.a.place.starts.from.visits.or = un luogo ha origine da visite o permanenze in due o più date, tutte entro 50 m l'una dall'altra e a più di 100 m da ogni altro luogo. quelle successive entro 50 m da esso si uniscono a esso. una permanenza o una visita con un margine di errore oltre 50 m non crea mai un luogo né si unisce mai a uno.

journal.window.map.visible.text.a.place.you.hid.isn.t = un luogo che hai nascosto non è disegnato. le sue visite compaiono ancora nell'elenco di ogni giorno.

journal.window.map.visible.text.a.stay.is.worked.out.from = una permanenza si ricava da letture precise, nessuna a più di cinque minuti dall'altra, che sono rimaste entro 100 m dalla prima di esse per almeno cinque minuti. le letture approssimative non creano mai una permanenza. i suoi orari sono la prima e l'ultima lettura.

journal.window.map.visible.text.a.stop.at.one.of.your = una fermata presso uno dei tuoi luoghi è disegnata dove si trova il luogo, così un luogo non è mai diviso tra due aree. una fermata il cui margine di errore non è stato segnalato, o è oltre 500 m, non è disegnata, e non è disegnato nemmeno il tempo tra le fermate.

journal.window.map.visible.text.also.in.this.area = anche in quest'area

journal.window.map.visible.text.checking.location.accuracy.before.using.it = si sta controllando la precisione della posizione prima di usarla. puoi continuare a usare il tuo diario.

journal.window.map.visible.text.days.here = giorni qui

journal.window.map.visible.text.days.with.a.stop = giorni con una fermata

journal.window.map.visible.text.each.area.is.about.km.across = ogni area è circa 1 km di lato e contiene le fermate al suo interno: le visite segnalate dal tuo telefono, e le permanenze che il tuo diario ha ricavato.

journal.window.map.visible.text.every.record.above.opens.in.transcripts = ogni registro qui sopra si apre nelle trascrizioni, dove elimini i suoi segmenti come qualsiasi altro. un segmento eliminato esce da questa vista e dalla ricerca.

journal.window.map.visible.text.filled.bars.are.visits.your.phone = le barre piene sono visite segnalate dal tuo telefono; le barre contornate sono permanenze che il tuo diario ha ricavato. ogni lettura è nella pagina di ogni giorno.

journal.window.map.visible.text.for.readings.the.times.are.the = per le letture, gli orari sono la prima e l'ultima lettura della riga. non dicono per quanto tempo eri da qualche parte.

journal.window.map.visible.text.hidden = nascosto

journal.window.map.visible.text.hours.are.shown.for.one.area = le ore sono mostrate per un'area alla volta, e solo per il tempo con entrambi gli estremi noti. non vengono sommate tra le aree, perché due segnalazioni possono mettere la stessa ora in due aree.

journal.window.map.visible.text.hours.with.both.ends.known = ore con entrambi gli estremi noti

journal.window.map.visible.text.location.accuracy.is.on = la precisione della posizione è attiva.

journal.window.map.visible.text.location.accuracy.is.unavailable = la precisione della posizione non è disponibile.

journal.window.map.visible.text.location.may.have.been.paused.or = la posizione può essere stata in pausa o disattivata, oppure il telefono può non essersi connesso al tuo diario. il tuo diario non può dire quale.

journal.window.map.visible.text.location.was.kept.this.day.but = la posizione è stata conservata in questo giorno, ma nessuna fermata.

journal.window.map.visible.text.named.features.only.streets.not.shown = solo elementi con nome; strade non mostrate.

journal.window.map.visible.text.nights = notti

journal.window.map.visible.text.no.location.kept.for.this.day = nessuna posizione conservata per questo giorno.

journal.window.map.visible.text.no.location.kept.for.this.month = nessuna posizione conservata per questo mese.

journal.window.map.visible.text.no.places.yet = ancora nessun luogo.

journal.window.map.visible.text.no.stops.to.draw.this.month = nessuna fermata da disegnare questo mese.

journal.window.map.visible.text.not.drawn.here = non disegnato qui:

journal.window.map.visible.text.nothing.drawn.here = · niente disegnato qui

journal.window.map.visible.text.on.the.map.a.run.of = sulla mappa, una serie di letture precise compare come un punto in corrispondenza della lettura di mezzo, così un giorno non si legge come un percorso. l'elenco conta ogni lettura.

journal.window.map.visible.text.saved = salvato.

journal.window.map.visible.text.shades.run.from.day.lightest.through = le tonalità vanno da 1 giorno, la più chiara, passando per 2 a 3, 4 a 7 e 8 a 14, fino a 15 o più giorni, la più scura.

journal.window.map.visible.text.show.fewer = mostra meno

journal.window.map.visible.text.stops = fermate

journal.window.map.visible.text.stops.in.order = fermate, in ordine

journal.window.map.visible.text.that.change.couldn.t.be.saved = non è stato possibile salvare la modifica.

journal.window.map.visible.text.the.shade.counts.days.not.hours = la tonalità conta i giorni, non le ore. un giorno conta una volta in un'area, per quante segnalazioni abbia, e una visita conta per il suo giorno anche quando l'arrivo o la partenza non è noto.

journal.window.map.visible.text.the.stops.your.journal.holds.for = le fermate che il tuo diario tiene per questo mese. ogni area è ombreggiata in base a quanti giorni hanno una fermata al suo interno.

journal.window.map.visible.text.there.s.no.street.map.a = non c'è una mappa stradale. una mappa stradale si carica pezzo per pezzo da un servizio di mappe, e chiedere il pezzo giusto dice a quel servizio dove eri.

journal.window.map.visible.text.there.s.no.street.map.the = non c'è una mappa stradale. i nomi sono quelli arrivati con il tuo diario o che hai scaricato, e non si chiede niente a nessun servizio di mappe.

journal.window.map.visible.text.to.remove.a.visit.or.stay = per rimuovere una visita o una permanenza, apri il suo giorno ed elimina i suoi segmenti nelle trascrizioni. un luogo dura quanto il segmento da cui proviene.

journal.window.map.visible.text.what.you.call.a.place.a = come chiami un luogo, un nome oppure casa o lavoro, si usa nella ricerca e nel ragionamento stesso del tuo diario, e un agente che può vedere le trascrizioni in tutto il tuo diario può leggerlo.

journal.window.map.visible.text.where = dove

journal.window.map.visible.text.where.starts.around.the.area.with = "where" inizia intorno all'area con più giorni: ogni fermata entro 50 km da essa. ogni serie di giorni con una fermata più lontana è un viaggio, chiamato in base alle sue date.

journal.window.map.visible.text.with.location.accuracy.a.visit.can = con la precisione della posizione, una visita può dire "vicino" ai luoghi entro il margine di errore del tuo telefono. ogni registro dice perché si legge in quel modo.

journal.window.map.visible.text.with.location.accuracy.a.visit.can.variant2 = con la precisione della posizione, una visita può dire "presso" un luogo quando l'intero margine di errore del tuo telefono è dentro il suo contorno, e "vicino" ai luoghi entro quel margine. "in" significa che l'intero margine è dentro un contorno più grande, un quartiere o una località. ogni registro dice perché si legge in quel modo.

journal.window.map.visible.text.without.it.a.visit.is.named = senza di essa, una visita è nominata dalla località vicina.

journal.window.map.visible.text.without.location.accuracy.each.reading.is = senza la precisione della posizione, ogni lettura prende il nome dalla località che meglio rappresenta la sua area, che non è sempre la località in cui eri. per questo dice "vicino".

journal.window.map.visible.text.you.can.name.a.place.call = puoi dare un nome a un luogo, chiamarlo casa o lavoro, oppure nasconderlo. nasconderlo tiene fuori dalla mappa tutto ciò che è entro 50 m da esso, e ciò che il tuo diario scrive su dove eri ogni giorno lo lascia fuori. l'elenco del giorno qui lo mostra ancora.

journal.window.map.visible.text.your.journal.couldn.t.read.the = non è stato possibile leggere la posizione conservata per questo giorno.

journal.window.map.visible.text.your.journal.draws.this.from.the = il tuo diario disegna questo dalle letture che il tuo telefono gli ha inviato, e dai nomi arrivati con il tuo diario o che hai scaricato. non si chiede niente a nessun servizio di mappe.

journal.window.map.visible.text.your.phone.often.reports.an.arrival = il tuo telefono spesso segnala prima un arrivo e più tardi l'intera visita. le due segnalazioni sono una visita.

journal.window.map.week.heading = la settimana di{ $slot1 }{ $date }

journal.window.map.week.readings.without.stops = { $quantity ->
    [one] { $count } lettura, nessuna visita o permanenza
    [many] { $count } di letture, nessuna visita o permanenza
   *[other] { $count } letture, nessuna visita o permanenza
    }

journal.window.map.window.explanation.a.place.in.your.journal = un luogo nel tuo diario

journal.window.map.window.explanation.a.segment.your.journal.couldn.t = un segmento che non è stato possibile leggere

journal.window.map.window.explanation.a.stay.your.journal.worked.out = una permanenza che il tuo diario ha ricavato

journal.window.map.window.explanation.a.visit.your.phone.reported = una visita segnalata dal tuo telefono

journal.window.map.window.explanation.approximate.location.each.reading.is.only = posizione approssimativa: ogni lettura è buona solo entro qualche chilometro, quanto basta per nominare la località, non l'edificio.

journal.window.map.window.explanation.choose.a.day.in.the.calendar = scegli un giorno nel calendario.

journal.window.map.window.explanation.choose.an.area.on.the.map = scegli un'area sulla mappa o nell'elenco, oppure un giorno nel calendario.

journal.window.map.window.explanation.each.bar.is.a.visit.your = ogni barra è una visita segnalata dal tuo telefono.

journal.window.map.window.explanation.hidden.from.the.map = nascosto dalla mappa

journal.window.map.window.explanation.in.the.chosen.area = nell'area scelta

journal.window.map.window.explanation.location.accuracy.couldn.t.be.downloaded = non è stato possibile scaricare la precisione della posizione. riprova tra un po'.

journal.window.map.window.explanation.location.accuracy.couldn.t.be.saved = non è stato possibile salvare la precisione della posizione nella cartella del tuo diario.

journal.window.map.window.explanation.location.accuracy.failed.its.file.check = la precisione della posizione non ha superato il controllo del file. scaricala di nuovo prima di usarla.

journal.window.map.window.explanation.location.comes.from.the.solstone.app = la posizione viene dall'app di solstone sul tuo telefono, dopo che l'attivi lì. ciò che prende va nel tuo diario e compare qui, giorno per giorno.

journal.window.map.window.explanation.location.couldn.t.be.read = non è stato possibile leggere la posizione

journal.window.map.window.explanation.map.fitted.to.these.areas = mappa adattata a queste aree

journal.window.map.window.explanation.margin.of.error.over.m.or = margine di errore oltre 500 m o non segnalato

journal.window.map.window.explanation.no.named.town.nearby = nessuna località nominata vicina

journal.window.map.window.explanation.open.it.in.transcripts.to.see = aprilo nelle trascrizioni per vederlo o eliminarlo

journal.window.map.window.explanation.the.area.with.the.most.days = l'area con più giorni

journal.window.map.window.explanation.this.trip.s.busiest.area = l'area più frequentata di questo viaggio

journal.window.map.window.explanation.this.version.of.your.journal.has = questa versione del tuo diario non ha uno scaricamento per la precisione della posizione.

journal.window.map.window.explanation.tied.for.the.most.days = in parità per il maggior numero di giorni

journal.window.map.window.explanation.where.two.reports.cover.the.same = dove due segnalazioni coprono lo stesso tempo, le ore contano quel tempo una volta.

journal.window.map.window.explanation.your.journal.couldn.t.read.part = non è stato possibile leggere parte della posizione di questo giorno.

journal.window.map.window.explanation.your.watch.gives.only.approximate.location = il tuo orologio dà solo una posizione approssimativa, quindi le sue letture indicano la località, non l'edificio.
