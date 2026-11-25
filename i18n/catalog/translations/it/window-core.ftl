journal.setup.cancel.admission = ammissione del programma di installazione non disponibile

journal.setup.cancel.attempt = tentativo del programma di installazione non disponibile

journal.setup.cancel.busy = il programma di installazione tiene ancora il suo lease

journal.setup.cancel.changed = tentativo di installazione cambiato

journal.setup.cancel.failed = non è stato possibile annullare

journal.setup.cancel.identity = identità del programma di installazione non disponibile

journal.setup.cancel.owner = titolare del programma di installazione non disponibile

journal.setup.cleanup.legacy.log = pulizia dell'alias di registro obsoleto non riuscita: { $error }

journal.setup.cleanup.retired.support = pulizia dello stato del portale di supporto dismesso non riuscita: { $error }

journal.setup.doctor.blocked = bloccante di doctor non riuscito

journal.setup.doctor.failed.status = doctor completato con stato { $status }

journal.setup.doctor.incomplete = il flusso JSONL di doctor è terminato senza { $event }

journal.setup.doctor.silent = doctor non ha segnalato progressi per { $seconds }s

journal.setup.doctor.timeout = doctor è scaduto dopo { $seconds }s

journal.setup.doctor.unreadable = analisi JSON di doctor non riuscita

journal.setup.downloads.cancel = annulla

journal.setup.downloads.cancelled = annullato

journal.setup.downloads.complete = pronto

journal.setup.downloads.error = non è stato possibile scaricare. riprova.

journal.setup.downloads.idle = non avviato

journal.setup.downloads.progress = scaricamento…

journal.setup.downloads.retry = riprova

journal.setup.downloads.speech = riconoscimento vocale

journal.setup.downloads.thinking = ragionamento in locale

journal.setup.drive.unsupported = il tuo diario non può funzionare su questo disco perché il disco non può salvare i file come serve al tuo diario. usa un disco formattato per mac per il tuo diario.

journal.setup.fits.app = funziona su ogni dispositivo che scegli e prende ciò che condividi con lei. tutto va nel tuo diario.

journal.setup.fits.app.term = l'app di { $platform_name }

journal.setup.fits.journal.computer = vive sul computer del tuo diario e tiene tutto ciò che entra nel tuo diario.

journal.setup.fits.journal.phone = vive su questo telefono e tiene tutto ciò che entra nel tuo diario.

journal.setup.fits.thinking = aiuta il tuo diario a dare un senso a ciò che tiene. scegli sotto dove avviene, o più tardi.

journal.setup.fits.thinking.term = ragionamento

journal.setup.fits.title = come si tiene insieme

journal.setup.folder.change = cambia cartella

journal.setup.folder.continue = continua

journal.setup.folder.help = dove il tuo diario è tenuto sul computer del tuo diario.

journal.setup.folder.title = cartella del diario

journal.setup.footer.computer = il tuo diario è tenuto sul computer del tuo diario. il ragionamento usa l'opzione che scegli.

journal.setup.footer.phone = il tuo diario è tenuto su questo telefono. il ragionamento usa l'opzione che scegli.

journal.setup.heading = il tuo diario

journal.setup.help.text = qualcosa non va, o hai una domanda?

journal.setup.identity.ca.invalid = CA locale non valida: { $detail }

journal.setup.identity.ca.randomness.unavailable = CSPRNG di sistema non disponibile

journal.setup.identity.foreign = non è stato possibile verificare questa installazione.{ $paragraph }dettagli: { $error }{ $paragraph }gli strumenti da riga di comando o il supporto in secondo piano su questo computer sono stati configurati da un'installazione diversa di { $platform }, quindi non è stato cambiato niente.

journal.setup.identity.invalid = stato del link locale non valido: { $detail }

journal.setup.identity.io.error = { $path }: { $error }

journal.setup.identity.locations = non è stato possibile verificare questa installazione.{ $paragraph }dettagli: { $error }{ $paragraph }le posizioni di ripristino della configurazione iniziale non sono disponibili: { $location_error }

journal.setup.identity.lock.timeout = non è stato possibile acquisire il blocco per { $path } entro { $seconds }s

journal.setup.identity.mark.asset.invalid = risorsa della marca non valida: { $detail }

journal.setup.identity.mark.jid.invalid = ID del diario non valido: { $detail }

journal.setup.identity.no.candidate = non esiste una candidata preparata per l'identità del link

journal.setup.identity.program.folder = Reinstallare o disinstallare rimuove questa cartella del programma. Scegli una posizione del diario al di fuori di essa. Se il tuo diario è già lì, esci dall'app del diario. Se la configurazione iniziale è completata, esegui `solstone journal service stop`, poi `solstone journal service uninstall` e attendi che riesca. Sposta l'intera cartella del diario fuori dalla cartella del programma prima di reinstallare o disinstallare, poi seleziona la nuova posizione con `solstone journal setup --accept-existing-journal --journal "D:\journal"`, sostituendo D:\journal con la nuova posizione.

journal.setup.identity.recover = non è stato possibile verificare questa installazione.{ $paragraph }dettagli: { $error }{ $paragraph }per il ripristino, ferma il servizio, rimuovi gli artefatti della configurazione iniziale di questa installazione ed esegui di nuovo `{ $setup }`:{ $stop_service }{ $steps }{ $paragraph }il tuo diario stesso non è stato toccato. nessuno di questi tiene i tuoi ricordi.

journal.setup.identity.spki.invalid = SPKI del diario non valido: { $detail }

journal.setup.identity.state.ca.bundle.is.not.a.directory = stato del link locale non valido: il bundle CA non è una directory

journal.setup.identity.state.candidate.ca.is.not.a.directory = stato del link locale non valido: la CA candidata non è una directory

journal.setup.identity.state.candidate.cert.pem = stato del link locale non valido: { $candidate_cert }: { $error }

journal.setup.identity.state.candidate.directory = stato del link locale non valido: directory candidata: { $error }

journal.setup.identity.state.candidate.private.pem = stato del link locale non valido: { $candidate_key }: { $error }

journal.setup.identity.state.cert.pem = stato del link locale non valido: { $cert_file }: { $error }

journal.setup.identity.state.identity.directory = stato del link locale non valido: directory dell'identità: { $error }

journal.setup.identity.state.private.pem = stato del link locale non valido: { $key_file }: { $error }

journal.setup.identity.state.state.instance.id.does.not.match.the.ca.public.key = stato del link locale non valido: lo stato { $identity_key } non corrisponde alla chiave pubblica della CA

journal.setup.identity.state.state.json = stato del link locale non valido: { $state_file }: { $error }

journal.setup.identity.state.state.json.instance.id.is.missing = stato del link locale non valido: { $state_file } { $identity_key } manca

journal.setup.identity.state.state.json.locked.at.is.missing = stato del link locale non valido: { $state_file } { $locked_key } manca

journal.setup.identity.state.state.json.must.be.an.object = stato del link locale non valido: { $state_file } deve essere un oggetto

journal.setup.identity.uncertain = non è stato possibile verificare questa installazione.{ $paragraph }dettagli: { $error }{ $paragraph }la configurazione iniziale non è riuscita a stabilire a quale installazione appartengono gli strumenti da riga di comando o il supporto in secondo piano. lasciali al loro posto e includi questi dettagli in una richiesta di supporto.

journal.setup.identity.windows = non è stato possibile verificare questa installazione.{ $paragraph }dettagli: { $error }{ $paragraph }per il ripristino, esegui prima `{ $uninstall }`. se non riesce, fermati qui e includi questi dettagli in una richiesta di supporto. se riesce, esegui questi comandi in { $shell }, poi esegui di nuovo `{ $setup }`:{ $commands }{ $paragraph }il tuo diario stesso non è stato toccato. nessuno di questi tiene i tuoi ricordi.

journal.setup.installer.cleanup.failed = pulizia dell'ammissione del programma di installazione non riuscita: { $error }

journal.setup.installer.control.timed.out = controllo del programma di installazione scaduto

journal.setup.installer.control.unavailable = controllo del programma di installazione non disponibile

journal.setup.installer.directory.unavailable = directory del programma di installazione non disponibile

journal.setup.installer.exited = il programma di installazione è uscito prima dell'ammissione ({ $code })

journal.setup.installer.identity.unavailable = identità del programma di installazione non disponibile

journal.setup.installer.job.authority = autorità Job del programma di installazione non disponibile

journal.setup.installer.job.cleanup = pulizia del Job del programma di installazione non riuscita: { $error }

journal.setup.installer.lease.busy = il lease del programma di installazione è occupato

journal.setup.installer.process.identity.invalid = identità del processo del programma di installazione non valida

journal.setup.installer.process.identity.unavailable = identità del processo del programma di installazione non disponibile

journal.setup.installer.process.observation.unavailable = osservazione del processo del programma di installazione non disponibile

journal.setup.installer.registry.unavailable = registro del programma di installazione non disponibile

journal.setup.installer.still.running = il programma di installazione è ancora in esecuzione

journal.setup.installer.stopped = il programma di installazione è stato fermato

journal.setup.installer.timed.out = ammissione del programma di installazione scaduta

journal.setup.installer.tree.not.reaped = albero dei processi non raccolto: { $reason }; superstiti={ $survivors }

journal.setup.journal.aborted = configurazione iniziale interrotta da te

journal.setup.journal.existing = { $setup }: impossibile procedere in modalità non interattiva - { $path } contiene già dati del diario.{ $line }La configurazione iniziale non rivendica automaticamente un diario esistente.{ $paragraph }Riprova con uno di questi:{ $indent }{ $accept }{ $indent }{ $new }{ $paragraph }Uscita interattiva:{ $indent }{ $verb }{ $paragraph }Esegui { $explain } per l'elenco completo dei passaggi.

journal.setup.journal.existing.installer = { $setup }: impossibile procedere in modalità non interattiva - { $path } contiene già dati del diario.{ $line }La configurazione iniziale non rivendica automaticamente un diario esistente.{ $paragraph }Riprova con uno di questi:{ $indent }{ $accept }{ $indent }{ $new }{ $paragraph }Uscita interattiva:{ $indent }{ $verb }{ $paragraph }Dopo uno qualsiasi di questi, esegui di nuovo lo stesso comando { $installer } per completare l'installazione.{ $paragraph }Esegui { $explain } per l'elenco completo dei passaggi.

journal.setup.journal.path.file = attesa una directory in { $path }; ottenuto un file regolare. Esegui di nuovo con { $option }.

journal.setup.login.help = si avvia in automatico dopo che accedi al computer del tuo diario.

journal.setup.login.label = avvia il tuo diario quando accedi

journal.setup.mark.error = non è stato possibile caricare la marca del tuo diario. riprova.

journal.setup.mark.help = una marca unica per il tuo diario. un telefono o un altro computer che accoppi la mostra anche, così puoi controllarla.

journal.setup.mark.permanence = continua rende questa la marca del tuo diario per sempre.

journal.setup.mark.title = la marca del tuo diario

journal.setup.mark.try.another = prova un'altra

journal.setup.refusal.action.is.required = un'azione è richiesta

journal.setup.refusal.attempt.id.is.required = { $attempt_id_value } è richiesto

journal.setup.refusal.bad.request = Richiesta non valida

journal.setup.refusal.choice.is.required = la scelta è richiesta

journal.setup.refusal.choose.local.thinking.before.starting.its.download = scegli il ragionamento in locale prima di avviare il suo scaricamento

journal.setup.refusal.choose.thinking.before.finishing.setup = scegli il ragionamento prima di finire la configurazione iniziale

journal.setup.refusal.clear.your.own.endpoint.first.to.run.the.bundled.local.model = cancella prima il tuo endpoint per eseguire il modello locale incluso.

journal.setup.refusal.conflict = Conflitto

journal.setup.refusal.forbidden = Vietato

journal.setup.refusal.identity.proposal.conflict = conflitto nella proposta di identità

journal.setup.refusal.internal.server.error = Errore interno del server

journal.setup.refusal.invalid.timezone = fuso orario non valido

journal.setup.refusal.journal.id.already.locked = id del diario già bloccato

journal.setup.refusal.journal.id.must.be.locked.before.setup.can.finish = l'id del diario deve essere bloccato prima che la configurazione iniziale possa finire

journal.setup.refusal.lane.must.be.one.of.byo.confidential.local = lane deve essere uno di: { $byo_value }, confidential, local

journal.setup.refusal.local.thinking.is.not.available.on.your.journal.s.computer = il ragionamento in locale non è disponibile sul computer del tuo diario

journal.setup.refusal.malformed.json.request = richiesta json malformata

journal.setup.refusal.malformed.lock.request = richiesta di blocco malformata

journal.setup.refusal.not.found = non trovato

journal.setup.refusal.owner.identity.failed = non è stato possibile salvare il tuo profilo del diario

journal.setup.refusal.proposal.id.is.required = { $proposal_id_value } è richiesto

journal.setup.refusal.retention.days.must.be.a.positive.integer = retention_days deve essere un intero positivo

journal.setup.refusal.retry.continues.the.same.backup.restore = il nuovo tentativo prosegue lo stesso ripristino del backup.

journal.setup.refusal.service.unavailable = Servizio non disponibile

journal.setup.refusal.settings.lock.unavailable = blocco delle impostazioni non disponibile

journal.setup.refusal.settings.read.failed = non è stato possibile leggere le impostazioni

journal.setup.refusal.settings.write.failed = non è stato possibile scrivere le impostazioni

journal.setup.refusal.setup.routes.require.a.localhost.connection = i percorsi della configurazione iniziale richiedono una connessione localhost

journal.setup.refusal.setup.target.use.was.refused = l'uso della destinazione della configurazione iniziale è stato rifiutato

journal.setup.refusal.speech.recognition.needs.repair = il riconoscimento vocale va riparato

journal.setup.refusal.the.restore.didn.t.complete = il ripristino non si è completato

journal.setup.refusal.thinking.choice.is.required.before.setup.can.finish = la scelta del ragionamento è richiesta prima che la configurazione iniziale possa finire

journal.setup.refusal.thinking.download.could.not.start = non è stato possibile avviare lo scaricamento del ragionamento

journal.setup.refusal.this.journal.is.still.being.restored.and.isn.t.ready.to.finish.setup = questo diario è ancora in ripristino e non è pronto a finire la configurazione iniziale.

journal.setup.refusal.this.journal.is.still.being.restored.and.isn.t.ready.to.use = questo diario è ancora in ripristino e non è pronto per l'uso.

journal.setup.refusal.this.journal.isn.t.ready.to.use = questo diario non è pronto per l'uso.

journal.setup.refusal.unknown.action = azione sconosciuta

journal.setup.refusal.unknown.thinking.choice = scelta di ragionamento sconosciuta

journal.setup.refusal.whole.journal.restore.is.pending = il ripristino dell'intero diario è in sospeso

journal.setup.refusal.your.journal.s.identity.couldn.t.be.set.up = non è stato possibile configurare l'identità del tuo diario.

journal.setup.refusal.your.journal.settings.could.not.be.read = non è stato possibile leggere le impostazioni del tuo diario

journal.setup.refusal.your.thinking.choice.changed.review.it.before.trying.again = la tua scelta di ragionamento è cambiata; rivedila prima di riprovare

journal.setup.refusal.your.thinking.settings.changed.open.thinking.to.choose.what.to.use = le tue impostazioni di ragionamento sono cambiate; apri il ragionamento per scegliere cosa usare

journal.setup.service.failed = avvio del servizio non riuscito (uscita { $status })

journal.setup.speech.cannot.start = non è stato possibile avviare l'installazione del parlato

journal.setup.speech.check.progress = controlla tra un momento l'avanzamento del parlato.

journal.setup.speech.retry.unavailable = il nuovo tentativo del parlato non è disponibile.

journal.setup.speech.starting = l'installazione del parlato si sta avviando

journal.setup.speech.unavailable = nuovo tentativo non disponibile

journal.setup.step.exit = passaggio { $step } uscito con codice { $status }

journal.setup.step.failed = passaggio non riuscito

journal.setup.step.timeout = passaggio { $step } scaduto dopo { $seconds }s

journal.setup.step.warning = avviso del passaggio

journal.setup.thinking.change = cambia

journal.setup.thinking.check.unknown = non è stato possibile controllare la configurazione locale.

journal.setup.thinking.choose.later = scegli il ragionamento quando vuoi. il tuo diario tiene tutto fino ad allora.

journal.setup.thinking.default.line = sul computer del tuo diario · { $size }

journal.setup.thinking.later = decidi più tardi

journal.setup.thinking.size.unknown = dimensione dello scaricamento non disponibile

journal.setup.welcome.body = l'app di { $platform_name } prende ciò che condividi con lei, e tutto va nel tuo diario.

journal.setup.welcome.title = ti diamo il benvenuto in { $platform_name }.

journal.setup.wrapper.legacy = il servizio si avvia ancora tramite { $path }

journal.setup.wrapper.legacy.retry = la configurazione iniziale rimuove quell'involucro dopo che il servizio si avvia tramite { $path }.

journal.setup.wrapper.provision = non è stato possibile predisporre l'involucro di { $command } in { $path } ({ $kind }: { $error })

journal.setup.wrapper.provision.remedy = correggi i permessi su ~/.local/bin ed esegui di nuovo `solstone journal setup`, oppure invoca solstone direttamente dall'ambiente di esecuzione

journal.setup.wrapper.retire.failed = non è stato possibile rimuovere gli strumenti da riga di comando della versione precedente: { $error }

journal.setup.wrapper.retry = la configurazione iniziale riprova alla prossima esecuzione.

journal.window.body.accessible.name.body.views = viste del corpo

journal.window.body.accessible.name.close = chiudi

journal.window.body.accessible.name.days.your.journal.holds.body.data = giorni in cui il tuo diario tiene dati del corpo

journal.window.body.accessible.name.empty.day = giorno vuoto

journal.window.body.button.label.delete = elimina

journal.window.body.count.day = { $quantity ->
    [one] { $count } giorno
    [many] { $count } di giorni
   *[other] { $count } giorni
}

journal.window.body.count.entry = { $quantity ->
    [one] { $count } elemento
    [many] { $count } di elementi
   *[other] { $count } elementi
}

journal.window.body.count.interval = { $quantity ->
    [one] { $count } intervallo
    [many] { $count } di intervalli
   *[other] { $count } intervalli
}

journal.window.body.count.reading = { $quantity ->
    [one] { $count } lettura
    [many] { $count } di letture
   *[other] { $count } letture
}

journal.window.body.date.clock.afternoon = { $hour }:{ $minute } PM

journal.window.body.date.clock.morning = { $hour }:{ $minute } AM

journal.window.body.date.clock.range = { $first_clock }–{ $last_clock }

journal.window.body.date.full = {$day}{ $slot1 }{$month}{ $slot2 }{$year}

journal.window.body.date.month.year = {$month}{ $slot1 }{$year}

journal.window.body.date.short = {$day}{ $slot1 }{$month}

journal.window.body.date.time.before = {$date},{ $slot1 }{$time}

journal.window.body.date.with.clock = {$day}{ $slot1 }{$month}{ $slot2 }{$year},{ $slot3 }{$time}

journal.window.body.day.invalid = non è stato possibile usare quel giorno.

journal.window.body.day.invalid.detail = Giorno non valido

journal.window.body.day.nearby.with.data = giorni vicini con dati: { $nearby_days }

journal.window.body.document.dated = per questa data: { $name } { $value }

journal.window.body.document.dated.list = { $quantity ->
    [one] documento giornaliero di { $source } per questa data: { $leading } e { $last }
    [many] documenti giornalieri di { $source } per questa data: { $leading } e { $last }
   *[other] documenti giornalieri di { $source } per questa data: { $leading } e { $last }
}

journal.window.body.document.dated.one = { $quantity ->
    [one] documento giornaliero di { $source } per questa data: { $last }
    [many] documenti giornalieri di { $source } per questa data: { $last }
   *[other] documenti giornalieri di { $source } per questa data: { $last }
}

journal.window.body.document.experience.continuing.generic = esperienza che continua: { $name }, { $start } a { $end }

journal.window.body.document.experience.continuing.generic.recorder = esperienza che continua: { $name }, { $start } a { $end } ({ $recorder })

journal.window.body.document.experience.continuing.named = { $kind } che continua: { $name }, { $start } a { $end }

journal.window.body.document.experience.continuing.named.recorder = { $kind } che continua: { $name }, { $start } a { $end } ({ $recorder })

journal.window.body.document.experience.generic = esperienza: { $name }, { $start } a { $end }

journal.window.body.document.experience.generic.recorder = esperienza: { $name }, { $start } a { $end } ({ $recorder })

journal.window.body.document.experience.generic.recorder.words = esperienza: { $name }, { $start } a { $end } ({ $recorder }): { $words }

journal.window.body.document.experience.generic.words = esperienza: { $name }, { $start } a { $end }: { $words }

journal.window.body.document.experience.named = { $kind }: { $name }, { $start } a { $end }

journal.window.body.document.experience.named.recorder = { $kind }: { $name }, { $start } a { $end } ({ $recorder })

journal.window.body.document.experience.named.recorder.words = { $kind }: { $name }, { $start } a { $end } ({ $recorder }): { $words }

journal.window.body.document.experience.named.words = { $kind }: { $name }, { $start } a { $end }: { $words }

journal.window.body.document.heading = corpo di { $weekday } { $day } { $month } { $year }

journal.window.body.document.heading.raw = corpo del { $day }

journal.window.body.document.tally.count.plain.at = { $quantity ->
    [one] { $name }: { $count } intervallo, { $start }
    [many] { $name }: { $count } di intervalli, { $start }
   *[other] { $name }: { $count } intervalli, { $start }
}

journal.window.body.document.tally.count.plain.count = { $quantity ->
    [one] { $name }: { $count } intervallo
    [many] { $name }: { $count } di intervalli
   *[other] { $name }: { $count } intervalli
}

journal.window.body.document.tally.count.plain.range = { $quantity ->
    [one] { $name }: { $count } intervallo, { $start } a { $end }
    [many] { $name }: { $count } di intervalli, { $start } a { $end }
   *[other] { $name }: { $count } intervalli, { $start } a { $end }
}

journal.window.body.document.tally.count.recorder.at = { $quantity ->
    [one] { $name }: { $count } intervallo, { $start } ({ $recorder })
    [many] { $name }: { $count } di intervalli, { $start } ({ $recorder })
   *[other] { $name }: { $count } intervalli, { $start } ({ $recorder })
}

journal.window.body.document.tally.count.recorder.count = { $quantity ->
    [one] { $name }: { $count } intervallo ({ $recorder })
    [many] { $name }: { $count } di intervalli ({ $recorder })
   *[other] { $name }: { $count } intervalli ({ $recorder })
}

journal.window.body.document.tally.count.recorder.range = { $quantity ->
    [one] { $name }: { $count } intervallo, { $start } a { $end } ({ $recorder })
    [many] { $name }: { $count } di intervalli, { $start } a { $end } ({ $recorder })
   *[other] { $name }: { $count } intervalli, { $start } a { $end } ({ $recorder })
}

journal.window.body.document.tally.logged.owner.at = { $quantity ->
    [one] { $name }: { $count } elemento, { $start } (inserito da te)
    [many] { $name }: { $count } di elementi, { $start } (inserito da te)
   *[other] { $name }: { $count } elementi, { $start } (inserito da te)
}

journal.window.body.document.tally.logged.owner.count = { $quantity ->
    [one] { $name }: { $count } elemento (inserito da te)
    [many] { $name }: { $count } di elementi (inserito da te)
   *[other] { $name }: { $count } elementi (inserito da te)
}

journal.window.body.document.tally.logged.owner.range = { $quantity ->
    [one] { $name }: { $count } elemento, { $start } a { $end } (inserito da te)
    [many] { $name }: { $count } di elementi, { $start } a { $end } (inserito da te)
   *[other] { $name }: { $count } elementi, { $start } a { $end } (inserito da te)
}

journal.window.body.document.tally.logged.owner.recorder.at = { $quantity ->
    [one] { $name }: { $count } elemento, { $start } (inserito da te in { $recorder })
    [many] { $name }: { $count } di elementi, { $start } (inserito da te in { $recorder })
   *[other] { $name }: { $count } elementi, { $start } (inserito da te in { $recorder })
}

journal.window.body.document.tally.logged.owner.recorder.count = { $quantity ->
    [one] { $name }: { $count } elemento (inserito da te in { $recorder })
    [many] { $name }: { $count } di elementi (inserito da te in { $recorder })
   *[other] { $name }: { $count } elementi (inserito da te in { $recorder })
}

journal.window.body.document.tally.logged.owner.recorder.range = { $quantity ->
    [one] { $name }: { $count } elemento, { $start } a { $end } (inserito da te in { $recorder })
    [many] { $name }: { $count } di elementi, { $start } a { $end } (inserito da te in { $recorder })
   *[other] { $name }: { $count } elementi, { $start } a { $end } (inserito da te in { $recorder })
}

journal.window.body.document.tally.logged.plain.at = { $quantity ->
    [one] { $name }: { $count } elemento, { $start }
    [many] { $name }: { $count } di elementi, { $start }
   *[other] { $name }: { $count } elementi, { $start }
}

journal.window.body.document.tally.logged.plain.count = { $quantity ->
    [one] { $name }: { $count } elemento
    [many] { $name }: { $count } di elementi
   *[other] { $name }: { $count } elementi
}

journal.window.body.document.tally.logged.plain.range = { $quantity ->
    [one] { $name }: { $count } elemento, { $start } a { $end }
    [many] { $name }: { $count } di elementi, { $start } a { $end }
   *[other] { $name }: { $count } elementi, { $start } a { $end }
}

journal.window.body.document.tally.logged.recorder.at = { $quantity ->
    [one] { $name }: { $count } elemento, { $start } (registrato da { $recorder })
    [many] { $name }: { $count } di elementi, { $start } (registrato da { $recorder })
   *[other] { $name }: { $count } elementi, { $start } (registrato da { $recorder })
}

journal.window.body.document.tally.logged.recorder.count = { $quantity ->
    [one] { $name }: { $count } elemento (registrato da { $recorder })
    [many] { $name }: { $count } di elementi (registrato da { $recorder })
   *[other] { $name }: { $count } elementi (registrato da { $recorder })
}

journal.window.body.document.tally.logged.recorder.range = { $quantity ->
    [one] { $name }: { $count } elemento, { $start } a { $end } (registrato da { $recorder })
    [many] { $name }: { $count } di elementi, { $start } a { $end } (registrato da { $recorder })
   *[other] { $name }: { $count } elementi, { $start } a { $end } (registrato da { $recorder })
}

journal.window.body.document.tally.reading.plain.at = { $quantity ->
    [one] { $name }: { $count } lettura, { $start }
    [many] { $name }: { $count } di letture, { $start }
   *[other] { $name }: { $count } letture, { $start }
}

journal.window.body.document.tally.reading.plain.count = { $quantity ->
    [one] { $name }: { $count } lettura
    [many] { $name }: { $count } di letture
   *[other] { $name }: { $count } letture
}

journal.window.body.document.tally.reading.plain.range = { $quantity ->
    [one] { $name }: { $count } lettura, { $start } a { $end }
    [many] { $name }: { $count } di letture, { $start } a { $end }
   *[other] { $name }: { $count } letture, { $start } a { $end }
}

journal.window.body.document.tally.reading.recorder.at = { $quantity ->
    [one] { $name }: { $count } lettura, { $start } ({ $recorder })
    [many] { $name }: { $count } di letture, { $start } ({ $recorder })
   *[other] { $name }: { $count } letture, { $start } ({ $recorder })
}

journal.window.body.document.tally.reading.recorder.count = { $quantity ->
    [one] { $name }: { $count } lettura ({ $recorder })
    [many] { $name }: { $count } di letture ({ $recorder })
   *[other] { $name }: { $count } letture ({ $recorder })
}

journal.window.body.document.tally.reading.recorder.range = { $quantity ->
    [one] { $name }: { $count } lettura, { $start } a { $end } ({ $recorder })
    [many] { $name }: { $count } di letture, { $start } a { $end } ({ $recorder })
   *[other] { $name }: { $count } letture, { $start } a { $end } ({ $recorder })
}

journal.window.body.document.tally.stage.plain.at = { $quantity ->
    [one] fase del sonno { $name }: { $count } intervallo, { $start }
    [many] fase del sonno { $name }: { $count } di intervalli, { $start }
   *[other] fase del sonno { $name }: { $count } intervalli, { $start }
}

journal.window.body.document.tally.stage.plain.count = { $quantity ->
    [one] fase del sonno { $name }: { $count } intervallo
    [many] fase del sonno { $name }: { $count } di intervalli
   *[other] fase del sonno { $name }: { $count } intervalli
}

journal.window.body.document.tally.stage.plain.range = { $quantity ->
    [one] fase del sonno { $name }: { $count } intervallo, { $start } a { $end }
    [many] fase del sonno { $name }: { $count } di intervalli, { $start } a { $end }
   *[other] fase del sonno { $name }: { $count } intervalli, { $start } a { $end }
}

journal.window.body.document.tally.stage.recorder.at = { $quantity ->
    [one] fase del sonno { $name }: { $count } intervallo, { $start } ({ $recorder })
    [many] fase del sonno { $name }: { $count } di intervalli, { $start } ({ $recorder })
   *[other] fase del sonno { $name }: { $count } intervalli, { $start } ({ $recorder })
}

journal.window.body.document.tally.stage.recorder.count = { $quantity ->
    [one] fase del sonno { $name }: { $count } intervallo ({ $recorder })
    [many] fase del sonno { $name }: { $count } di intervalli ({ $recorder })
   *[other] fase del sonno { $name }: { $count } intervalli ({ $recorder })
}

journal.window.body.document.tally.stage.recorder.range = { $quantity ->
    [one] fase del sonno { $name }: { $count } intervallo, { $start } a { $end } ({ $recorder })
    [many] fase del sonno { $name }: { $count } di intervalli, { $start } a { $end } ({ $recorder })
   *[other] fase del sonno { $name }: { $count } intervalli, { $start } a { $end } ({ $recorder })
}

journal.window.body.document.words.owner = nelle tue parole: { $words }

journal.window.body.document.words.source = nelle parole della fonte: { $words }

journal.window.body.empty.workouts.import = qui non c'è ancora niente. porta i tuoi allenamenti da un download di Strava nella { $import_link_start }pagina di importazione{ $import_link_end }.

journal.window.body.empty.workouts.import.resources = se il tuo diario funziona su un computer, puoi anche portare un'esportazione di Apple Health nella pagina di importazione o i dati di Oura dalla riga di comando di quel computer. { $docs_link_start }leggi sulle importazioni del corpo{ $docs_link_end }.

journal.window.body.error.day.detail = Giorno non valido

journal.window.body.error.day.invalid = non è stato possibile usare quel giorno.

journal.window.body.error.hidden.body = Il corpo deve essere un oggetto JSON

journal.window.body.error.hidden.days.empty = i giorni devono indicare almeno un giorno

journal.window.body.error.hidden.days.list = i giorni devono essere un elenco di giorni

journal.window.body.error.hidden.hidden = nascosto deve essere vero o falso

journal.window.body.error.hidden.unreadable = il tuo diario non può leggere quali giorni hai nascosto.

journal.window.body.error.hidden.unreadable.detail = i giorni del corpo nascosti non si possono leggere

journal.window.body.error.hidden.unsaved = non è stato possibile salvare la modifica.

journal.window.body.error.invalid.request = non è stato possibile usare uno di quei valori.

journal.window.body.error.limit.integer = il limite deve essere un intero

journal.window.body.error.limit.minimum = il limite deve essere almeno 1

journal.window.body.error.month.invalid = Mese non valido

journal.window.body.error.unreadable = non è stato possibile leggere il tuo diario dall'app del corpo.

journal.window.body.error.window.bounds = La finestra richiede timestamp ISO validi per l'inizio e la fine.

journal.window.body.error.window.order = La fine della finestra deve essere dopo l'inizio della finestra.

journal.window.body.error.window.span = { $quantity ->
    [one] L'intervallo della finestra deve essere di { $maximum_days } giorno o meno.
    [many] L'intervallo della finestra deve essere di { $maximum_days } di giorni o meno.
   *[other] L'intervallo della finestra deve essere di { $maximum_days } giorni o meno.
}

journal.window.body.experience.continuing.generic = esperienza che continua

journal.window.body.experience.continuing.named = { $experience_kind } che continua

journal.window.body.experience.fallback = esperienza

journal.window.body.grid.empty = {$date}{ $slot1 }· qui non c'è niente

journal.window.body.grid.held = {$date}{ $slot1 }· trattenuto in un'importazione precedente

journal.window.body.grid.held.month = {$date}{ $slot1 }· in un mese che un'importazione precedente tiene

journal.window.body.grid.hidden = {$date}{ $slot1 }· hai nascosto questo giorno

journal.window.body.grid.source.all = { $date } · { $slot1 }, { $slot2 } e { $slot3 }

journal.window.body.grid.source.apple = { $date } · { $slot1 }

journal.window.body.grid.source.apple.oura = { $date } · { $slot1 } e { $slot2 }

journal.window.body.grid.source.apple.strava = { $date } · { $slot1 } e { $slot2 }

journal.window.body.grid.source.oura = { $date } · { $slot1 }

journal.window.body.grid.source.oura.strava = { $date } · { $slot1 } e { $slot2 }

journal.window.body.grid.source.strava = { $date } · { $slot1 }

journal.window.body.hide.copy.grid.foot.striped.days.are.ones.you.hid = i giorni a strisce sono quelli che hai nascosto.

journal.window.body.hide.copy.hide.failed.couldn.t.hide.this.day.nothing = non è stato possibile nascondere questo giorno. niente è cambiato.

journal.window.body.hide.copy.hide.hide.this.day = nascondi questo giorno

journal.window.body.hide.copy.mark.head.you.hid.this.day.s.body = hai nascosto i dati del corpo di questo giorno

journal.window.body.hide.copy.mark.line1.it.s.still.in.your.journal = è ancora nel tuo diario, e resta nascosto quando lo importi o lo sincronizzi di nuovo.

journal.window.body.hide.copy.mark.line2.search.and.the.activities.page.leave = la ricerca e la pagina delle attività lo escludono, e gli agenti connessi non possono connettersi a questi dati. una notte che inizia in questo giorno è nascosta con esso, anche dopo mezzanotte.

journal.window.body.hide.copy.mark.unhide.unhide.this.day = mostra di nuovo questo giorno

journal.window.body.hide.copy.no.answer.hide.it.may.already.be.hidden.reload = potrebbe essere già nascosto. ricarica per controllare.

journal.window.body.hide.copy.no.answer.unhide.it.may.already.be.unhidden.reload = potrebbe essere già visibile di nuovo. ricarica per controllare.

journal.window.body.hide.copy.no.answer.your.journal.didn.t.answer = il tuo diario non ha risposto

journal.window.body.hide.copy.receipt.message.it.stays.hidden.when.you.import = resta nascosto quando lo importi o lo sincronizzi di nuovo.

journal.window.body.hide.copy.receipt.no.search.search.and.connected.agents.may.still = la ricerca e gli agenti connessi potrebbero ancora trovarlo.

journal.window.body.hide.copy.try.again.try.again = riprova

journal.window.body.hide.copy.unhide.failed.couldn.t.unhide.this.day.nothing = non è stato possibile mostrare di nuovo questo giorno. niente è cambiato.

journal.window.body.hide.copy.unhide.unhide = mostra di nuovo

journal.window.body.hide.copy.unreadable.your.journal.can.t.read.which = il tuo diario non può leggere quali giorni hai nascosto, quindi qui compare ogni giorno e non puoi nasconderne uno né mostrarlo di nuovo. finché non può, la ricerca e gli agenti connessi escludono tutti i tuoi dati del corpo.

journal.window.body.hide.data.from.day = nascondi i dati del corpo da { $day }

journal.window.body.hide.data.receipt = i dati del corpo da { $day } sono nascosti.

journal.window.body.hide.previous.day.carryover = il resto di una notte o di un allenamento da { $hidden_day_link }, un giorno che hai nascosto, è nascosto anche qui.

journal.window.body.import.again.apple.health = importa di nuovo da una nuova esportazione di Apple Health per portarlo dentro.

journal.window.body.import.again.dexcom = non c'è un importatore per Dexcom Clarity ora, quindi resta com'è.

journal.window.body.import.again.oura = importa di nuovo dal tuo account Oura per portarlo dentro.

journal.window.body.import.old.day = un'importazione{ $slot1 }{$source}{ $slot2 }precedente tiene i dati del corpo di questo giorno, conservati ma non letti qui.{ $slot3 }{$action}

journal.window.body.import.old.listing = {$source}{ $slot1 }da{ $slot2 }{$span}, conservata ma non letta qui.{ $slot3 }{$action}

journal.window.body.import.old.month = un'importazione{ $slot1 }{$source}{ $slot2 }precedente tiene i dati del corpo per{ $slot3 }{$month}, conservati ma non letti qui, quindi non compare in questa pagina.{ $slot4 }{$action}

journal.window.body.import.progress.running.days = { $total_quantity ->
    [one] { $done_quantity ->
        [one] ancora in esecuzione: { $done_days } di { $total_days } giorno fatto.
        [many] ancora in esecuzione: { $done_days } di { $total_days } giorno fatto.
       *[other] ancora in esecuzione: { $done_days } di { $total_days } giorno fatto.
    }
    [many] { $done_quantity ->
        [one] ancora in esecuzione: { $done_days } di { $total_days } di giorni fatti.
        [many] ancora in esecuzione: { $done_days } di { $total_days } di giorni fatti.
       *[other] ancora in esecuzione: { $done_days } di { $total_days } di giorni fatti.
    }
   *[other] { $done_quantity ->
        [one] ancora in esecuzione: { $done_days } di { $total_days } giorni fatti.
        [many] ancora in esecuzione: { $done_days } di { $total_days } giorni fatti.
       *[other] ancora in esecuzione: { $done_days } di { $total_days } giorni fatti.
    }
}

journal.window.body.import.progress.running.empty = ancora in esecuzione, e non c'è ancora niente di salvato.

journal.window.body.import.progress.running.entries = { $quantity ->
    [one] ancora in esecuzione: { $entry_count } elemento letto finora.
    [many] ancora in esecuzione: { $entry_count } di elementi letti finora.
   *[other] ancora in esecuzione: { $entry_count } elementi letti finora.
}

journal.window.body.import.progress.stopped.days = { $total_quantity ->
    [one] { $done_quantity ->
        [one] fermata a metà: { $done_days } di { $total_days } giorno fatto.
        [many] fermata a metà: { $done_days } di { $total_days } giorno fatto.
       *[other] fermata a metà: { $done_days } di { $total_days } giorno fatto.
    }
    [many] { $done_quantity ->
        [one] fermata a metà: { $done_days } di { $total_days } di giorni fatti.
        [many] fermata a metà: { $done_days } di { $total_days } di giorni fatti.
       *[other] fermata a metà: { $done_days } di { $total_days } di giorni fatti.
    }
   *[other] { $done_quantity ->
        [one] fermata a metà: { $done_days } di { $total_days } giorni fatti.
        [many] fermata a metà: { $done_days } di { $total_days } giorni fatti.
       *[other] fermata a metà: { $done_days } di { $total_days } giorni fatti.
    }
}

journal.window.body.import.progress.stopped.empty = fermata a metà, e non c'è ancora niente di salvato.

journal.window.body.import.progress.stopped.entries = { $quantity ->
    [one] fermata a metà: { $entry_count } elemento letto finora.
    [many] fermata a metà: { $entry_count } di elementi letti finora.
   *[other] fermata a metà: { $entry_count } elementi letti finora.
}

journal.window.body.import.span.counted.days = { $quantity ->
    [one] { $first } a { $last }, { $count } giorno
    [many] { $first } a { $last }, { $count } di giorni
   *[other] { $first } a { $last }, { $count } giorni
}

journal.window.body.import.span.counted.months = { $quantity ->
    [one] { $first } a { $last } ({ $count } mese)
    [many] { $first } a { $last } ({ $count } di mesi)
   *[other] { $first } a { $last } ({ $count } mesi)
}

journal.window.body.import.span.month = {$month}{ $slot1 }(per mese)

journal.window.body.import.span.unknown = un intervallo sconosciuto

journal.window.body.link.label.body.overview = panoramica del corpo →

journal.window.body.link.label.body.overview.variant2 = ← panoramica del corpo

journal.window.body.link.label.body.overview.variant3 = panoramica del corpo

journal.window.body.logged.who.by = tu, in{ $slot1 }{$source}

journal.window.body.logged.who.you = tu

journal.window.body.message.it.may.already.be.back.in = potrebbe essere già di nuovo nel tuo diario. ricarica per controllare.

journal.window.body.message.it.may.already.be.erased.reload = potrebbe essere già cancellato. ricarica per controllare.

journal.window.body.message.it.may.already.be.in.the = potrebbe essere già nel cestino. ricarica per controllare.

journal.window.body.message.it.s.back.in.your.journal = è di nuovo nel tuo diario.

journal.window.body.message.it.s.in.the.trash.you = è nel cestino. puoi riprovare da lì.

journal.window.body.message.it.s.still.in.the.trash = è ancora nel cestino. puoi riprovare da lì.

journal.window.body.message.it.s.still.in.your.journal = è ancora nel tuo diario.

journal.window.body.section.heading.counts = conteggi

journal.window.body.section.heading.earlier.imports.kept.but.not.read = importazioni precedenti, conservate ma non lette qui

journal.window.body.section.heading.every.day.held = ogni giorno trattenuto

journal.window.body.section.heading.for.this.date = per questa data

journal.window.body.section.heading.imports = importazioni

journal.window.body.section.heading.logged = annotati

journal.window.body.section.heading.readings = letture

journal.window.body.section.heading.sources.held = fonti trattenute

journal.window.body.section.heading.strava = Strava

journal.window.body.sleep.stage = fase del sonno{ $slot1 }{$stage}

journal.window.body.status.text.couldn.t.load.body.day = non è stato possibile caricare il giorno del corpo

journal.window.body.status.text.couldn.t.load.your.body.data = non è stato possibile caricare i tuoi dati del corpo

journal.window.body.strava.commute = casa-lavoro

journal.window.body.strava.distance = {$distance}{ $slot1 }km

journal.window.body.strava.duration.hours = {$hours}{ $slot1 }h

journal.window.body.strava.duration.minutes = {$minutes}{ $slot1 }min

journal.window.body.strava.duration.time = {$hours}{ $slot1 }h{ $slot2 }{$minutes}{ $slot3 }min

journal.window.body.strava.elevation = dislivello{ $slot1 }{$metres}{ $slot2 }m

journal.window.body.strava.energy = {$value}{ $slot1 }kcal

journal.window.body.strava.heart = frequenza cardiaca media{ $slot1 }{$average}, max{ $slot2 }{$maximum}{ $slot3 }bpm

journal.window.body.strava.heart.average = frequenza cardiaca media{ $slot1 }{$value}{ $slot2 }bpm

journal.window.body.strava.heart.maximum = frequenza cardiaca max{ $slot1 }{$value}{ $slot2 }bpm

journal.window.body.strava.manual = inserito a mano

journal.window.body.strava.moving.hours = in movimento { $hours } h

journal.window.body.strava.moving.minutes = in movimento { $minutes } min

journal.window.body.strava.moving.time = in movimento { $hours } h { $minutes } min

journal.window.body.strava.power = potenza media{ $slot1 }{$average}{ $slot2 }W, ponderata{ $slot3 }{$weighted}{ $slot4 }W

journal.window.body.strava.power.average = potenza media{ $slot1 }{$value}{ $slot2 }W

journal.window.body.strava.power.weighted = potenza ponderata{ $slot1 }{$value}{ $slot2 }W

journal.window.body.strava.workout = allenamento

journal.window.body.title.couldn.t.erase.that.workout = non è stato possibile cancellare quell'allenamento

journal.window.body.title.couldn.t.move.that.to.the = non è stato possibile spostare quello nel cestino

journal.window.body.title.couldn.t.recover.that.workout = non è stato possibile ripristinare quell'allenamento

journal.window.body.title.delete.this.workout = elimina questo allenamento?

journal.window.body.title.erase.this.workout.now = cancella questo allenamento ora?

journal.window.body.title.moved.to.the.trash = spostato nel cestino

journal.window.body.title.workout.erased.from.your.journal = allenamento cancellato dal tuo diario

journal.window.body.title.workout.recovered = allenamento ripristinato

journal.window.body.title.your.journal.didn.t.answer = il tuo diario non ha risposto

journal.window.body.visible.text.nothing.for.this.day = niente per questo giorno.

journal.window.body.visible.text.part.of.this.day.couldn.t = non è stato possibile leggere una parte di questo giorno.

journal.window.body.visible.text.strava = · Strava

journal.window.body.visible.text.strava.variant2 = Strava

journal.window.body.window.explanation.your.body.brought.into.your.journal = il tuo corpo, portato nel tuo diario.

journal.window.body.window.state.text.loading.body.data = caricamento dei dati del corpo…

journal.window.body.window.state.text.loading.body.day = caricamento del giorno del corpo…

journal.window.body.workout.copy.again.importing.a.strava.download.again.brings = importare di nuovo un download di Strava riporta questo allenamento. un giorno che nascondi resta nascosto quando lo importi di nuovo.

journal.window.body.workout.copy.back.back = indietro

journal.window.body.workout.copy.cancel.cancel = annulla

journal.window.body.workout.copy.erase.now.erase.now = cancella ora

journal.window.body.workout.copy.move.to.trash.move.to.trash = sposta nel cestino

journal.window.body.workout.copy.open.trash.open.trash = apri il cestino

journal.window.body.workout.copy.reach.it.leaves.your.journal.now.and = esce ora dal tuo diario e non si può recuperare.

journal.window.body.workout.copy.reach.it.leaves.your.journal.now.and.variant2 = esce ora dal tuo diario e attende nel cestino, poi viene cancellato. puoi ripristinarlo fino ad allora.

journal.window.body.workout.copy.recover.recover = ripristina

journal.window.body.workout.copy.stays.activities.summaries.and.entities.made.from = attività, riepiloghi ed entità ricavati da essa altrove nel tuo diario non vengono cancellati e possono ancora menzionarla.

journal.window.body.workout.copy.stays.activities.summaries.and.entities.made.from.variant2 = attività, riepiloghi ed entità ricavati da essa altrove nel tuo diario restano dove sono e possono ancora menzionarla.

journal.window.body.workout.delete.name = elimina { $workout_name }

journal.window.body.workout.strava.additional.count = { $quantity ->
    [one] altro da Strava · { $count }
    [many] altro da Strava · { $count }
   *[other] altro da Strava · { $count }
}

journal.window.body.workout.strava.figures = le cifre di Strava: { $figures }

journal.window.body.workout.trash.recovers.until.day = lo cancella { $erase_day } a meno che non lo ripristini.

journal.window.calendar.note.added = Evento del calendario aggiunto

journal.window.calendar.note.added.titled = Evento del calendario aggiunto: { $title }

journal.window.calendar.note.attendee.owner = { $name }, tu

journal.window.calendar.note.attendee.pair = { $attendees }; { $attendee }

journal.window.calendar.note.attendee.status = { $name } ({ $status })

journal.window.calendar.note.calendar = Calendario: { $calendar }

journal.window.calendar.note.calendar.account = Calendario: { $calendar } ({ $account })

journal.window.calendar.note.changed = Evento del calendario cambiato

journal.window.calendar.note.changed.titled = Evento del calendario cambiato: { $title }

journal.window.calendar.note.date = { $weekday }, { $day } { $month } { $year }

journal.window.calendar.note.invited = Invitati: { $attendees }

journal.window.calendar.note.invited.more = { $remaining ->
    [one] Invitati: { $attendees }; e { $remaining } altro
    [many] Invitati: { $attendees }; e { $remaining } di altri
   *[other] Invitati: { $attendees }; e { $remaining } altri
}

journal.window.calendar.note.notes = Note: { $notes }

journal.window.calendar.note.recorded = Registrato: { $date }, { $time } ({ $offset })

journal.window.calendar.note.removed = Evento del calendario rimosso

journal.window.calendar.note.removed.titled = Evento del calendario rimosso: { $title }

journal.window.calendar.note.repeats = Si ripete: { $repeats }

journal.window.calendar.note.when.across.days = Quando: { $start_date }, { $start_time } a { $end_date }, { $end_time }

journal.window.calendar.note.when.across.days.zoned = Quando: { $start_date }, { $start_time } a { $end_date }, { $end_time } ({ $zone })

journal.window.calendar.note.when.all.day = Quando: { $date }, tutto il giorno

journal.window.calendar.note.when.all.days = Quando: { $start_date } a { $end_date }, tutto il giorno

journal.window.calendar.note.when.same.day = Quando: { $date }, { $start_time } a { $end_time }

journal.window.calendar.note.when.same.day.zoned = Quando: { $date }, { $start_time } a { $end_time } ({ $zone })

journal.window.calendar.note.when.time = Quando: { $date }, { $time }

journal.window.calendar.note.when.time.zoned = Quando: { $date }, { $time } ({ $zone })

journal.window.calendar.note.where = Dove: { $location }

journal.window.chrome.connection.connecting = connessione in corso…

journal.window.chrome.connection.delayed = alcune funzionalità potrebbero essere in ritardo

journal.window.chrome.connection.reconnecting = connessione persa. riconnessione in corso

journal.window.chrome.console.action.clear = svuota

journal.window.chrome.console.action.close = chiudi

journal.window.chrome.console.action.send = invia

journal.window.chrome.console.action.send.all = invia al supporto

journal.window.chrome.console.action.show.details = mostra i dettagli

journal.window.chrome.console.detail.location.label = posizione

journal.window.chrome.console.detail.message.label = messaggio

journal.window.chrome.console.detail.stack.label = stack

journal.window.chrome.console.empty = nessun messaggio di sistema in questa sessione.

journal.window.chrome.console.heading = messaggi di sistema

journal.window.chrome.console.link.label = messaggi di sistema

journal.window.chrome.console.reporting.off = l'invio delle segnalazioni è disattivo.

journal.window.chrome.console.summary.load.failed = non è stato possibile caricare una parte di questa pagina.

journal.window.chrome.console.summary.request.failed = non è stato possibile finire quella richiesta.

journal.window.chrome.console.summary.unexpected = qualcosa in questa pagina non ha funzionato.

journal.window.chrome.console.tab.all = tutti

journal.window.chrome.console.tab.errors = errori

journal.window.chrome.console.tab.info = info

journal.window.chrome.console.tab.warnings = avvisi

journal.window.chrome.day.zones.all = gli orari di questo giorno sono su { $zones }

journal.window.chrome.day.zones.part = alcuni orari di questo giorno sono su { $zones }

journal.window.chrome.log.read.failed = non è stato possibile leggere quel registro. ricarica per riprovare.

journal.window.chrome.reload.hint = ricarica per riprovare.

journal.window.chrome.report.action.cancel = annulla

journal.window.chrome.report.action.send = continua al supporto

journal.window.chrome.report.app.label = app

journal.window.chrome.report.button.label = segnala questo

journal.window.chrome.report.default.subject = errore di convey

journal.window.chrome.report.error.code.label = codice errore

journal.window.chrome.report.privacy.note = non esce niente da questo diario finché non invii il modulo sul sito di supporto.

journal.window.chrome.report.recent.label = righe di errore recenti

journal.window.chrome.report.route.label = percorso

journal.window.chrome.report.title = rivedi questa segnalazione

journal.window.chrome.retry = riprova

journal.window.chrome.section.error = non è stato possibile caricare questa sezione

journal.window.chrome.still.working = ancora in corso…

journal.window.chrome.unknown.error = qualcosa è andato storto. ricarica per riprovare.

journal.window.chrome.view.document.title = { $view ->
    [agents] agenti - journal
    [activities] attività - journal
    [backup] backup - journal
    [body] corpo - journal
    [curation] suggerimenti - journal
    [entities] entità - journal
    [health] stato - journal
    [home] inizio - journal
    [import] importazione - journal
    [map] mappa - journal
    [more] altro - journal
    [network] rete - journal
    [search] ricerca - journal
    [settings] impostazioni - journal
    [sources] fonti - journal
    [speakers] parlanti - journal
    [stats] statistiche - journal
    [support] supporto - journal
    [thinking] ragionamento - journal
    [transcripts] trascrizioni - journal
    [year] anno - journal
   *[other] { $fallback_label } - journal
}

journal.window.chrome.zone.time = ora di { $label }

journal.window.content.action.heading = ### { $action } da { $actor }

journal.window.content.action.parameters = **Parametri:**

journal.window.content.action.source = **Fonte:** { $source }

journal.window.content.action.talent = **Talento:** [{ $id }](/app/thinking/#runs/run/{ $id })

journal.window.content.action.time = **Ora:** { $time }

journal.window.content.action.unknown = sconosciuto

journal.window.content.activity.field.activity = - Attività: { $value }

journal.window.content.activity.field.day = - Giorno: { $value }

journal.window.content.activity.field.description = - Descrizione: { $value }

journal.window.content.activity.field.details = - Dettagli: { $value }

journal.window.content.activity.field.facet = - Facetta: { $value }

journal.window.content.activity.field.kind = - Tipo: { $value }

journal.window.content.activity.field.level = - Livello dell'ultimo segmento coinvolto: { $value }

journal.window.content.activity.field.time = - Ora: { $value }

journal.window.content.activity.hidden = - Nascosta: sì

journal.window.content.activity.omitted = Non tutto ciò che è stato salvato con questa attività è rientrato in questo elemento. Il registro completo dell'attività ha tutto.

journal.window.content.activity.role.attendee = - Partecipanti: { $names }

journal.window.content.activity.role.heard = - In conversazione: { $names }

journal.window.content.activity.role.mentioned = - Menzionati: { $names }

journal.window.content.activity.role.other = - Partecipazione: { $names }

journal.window.content.activity.role.visible = - Sullo schermo: { $names }

journal.window.content.activity.saved.relation = Relazione salvata: { $value }

journal.window.content.activity.topics = Argomenti: { $topics }

journal.window.content.activity.untitled = Attività senza titolo

journal.window.content.body.attribution = come { $source } l'ha registrato

journal.window.content.body.attribution.from = come { $source } l'ha registrato, da { $by }

journal.window.content.body.attribution.values = { $attribution }: { $values }

journal.window.content.body.heading = ## { $source } { $kind }: { $label }

journal.window.content.body.kind.default = esperienza

journal.window.content.body.span = { $start } a { $end }

journal.window.content.browser.app.paused = l'app di solstone in pausa

journal.window.content.browser.context = contesto: { $details }

journal.window.content.browser.duration.d = { $days } d

journal.window.content.browser.duration.dh = { $days } d { $hours } h

journal.window.content.browser.duration.h = { $hours } h

journal.window.content.browser.duration.hm = { $hours } h { $minutes } min

journal.window.content.browser.duration.m = { $minutes } min

journal.window.content.browser.duration.s = { $seconds } s

journal.window.content.browser.language = lingua { $lang }

journal.window.content.browser.media.playing = contenuti multimediali in riproduzione

journal.window.content.browser.media.playing.muted = contenuti multimediali in riproduzione, silenziati

journal.window.content.browser.page.default = Pagina del browser

journal.window.content.browser.tab.age = scheda aperta da almeno { $duration }

journal.window.content.browser.tab.background = scheda in secondo piano

journal.window.content.browser.tab.front = scheda davanti

journal.window.content.browser.tab.front.focused = scheda davanti, finestra attiva

journal.window.content.browser.tab.front.unfocused = scheda davanti, finestra non attiva

journal.window.content.chat.speaker.assistant = Assistente

journal.window.content.chat.speaker.human = Umano

journal.window.content.chat.speaker.unknown = Sconosciuto

journal.window.content.chat.speaker.you = Tu

journal.window.content.chat.turn = **{ $speaker }:** { $text }

journal.window.content.desktop.activity.field.app = app: { $value }

journal.window.content.desktop.activity.field.app.id = app_id: { $value }

journal.window.content.desktop.activity.field.capability = capacità: { $value }

journal.window.content.desktop.activity.field.cwd = cwd: { $value }

journal.window.content.desktop.activity.field.end = end_ms: { $value }

journal.window.content.desktop.activity.field.exe = eseguibile: { $value }

journal.window.content.desktop.activity.field.kind = tipo: { $value }

journal.window.content.desktop.activity.field.method = metodo: { $value }

journal.window.content.desktop.activity.field.pane.title = pane_title: { $value }

journal.window.content.desktop.activity.field.path = percorso: { $value }

journal.window.content.desktop.activity.field.session = sessione: { $value }

journal.window.content.desktop.activity.field.start = start_ms: { $value }

journal.window.content.desktop.activity.field.state = condizione: { $value }

journal.window.content.desktop.activity.field.status = stato: { $value }

journal.window.content.desktop.activity.field.stream = flusso: { $value }

journal.window.content.desktop.activity.field.title = titolo: { $value }

journal.window.content.desktop.activity.field.url = url: { $value }

journal.window.content.desktop.activity.field.window = finestra: { $value }

journal.window.content.desktop.activity.heading = attività sul desktop

journal.window.content.document.absent = Non specificato in questo documento

journal.window.content.document.asset.unnamed = Risorsa senza nome

journal.window.content.document.detail.applies = Si applica a: { $value }

journal.window.content.document.detail.timing = Tempistica: { $value }

journal.window.content.document.party.unnamed = Parte senza nome

journal.window.content.document.section.assets = Risorse e proprietà

journal.window.content.document.section.conditions = Condizioni e fattori scatenanti

journal.window.content.document.section.dates = Date importanti

journal.window.content.document.section.overview = Panoramica

journal.window.content.document.section.parties = Parti e ruoli

journal.window.content.document.section.provisions = Disposizioni principali

journal.window.content.document.section.summary = Riepilogo

journal.window.content.entity.aka = **Noto anche come:** { $items }

journal.window.content.entity.description.none = *(Nessuna descrizione disponibile)*

journal.window.content.entity.name.unnamed = Senza nome

journal.window.content.entity.tags = **Tag:** { $items }

journal.window.content.entity.type.unknown = Sconosciuto

journal.window.content.event.created = **Creato il:** { $date }

journal.window.content.event.heading.occurred = ### { $kind }: { $title }

journal.window.content.event.heading.planned = ### { $kind } in programma: { $title }

journal.window.content.event.participants.occurred = **Partecipanti:** { $names }

journal.window.content.event.participants.planned = **Partecipanti previsti:** { $names }

journal.window.content.event.time.occurred = **Quando è avvenuto:** { $start }

journal.window.content.event.time.occurred.range = **Quando è avvenuto:** { $start } - { $end }

journal.window.content.event.time.scheduled = **Orario previsto:** { $start }

journal.window.content.event.time.scheduled.range = **Orario previsto:** { $start } - { $end }

journal.window.content.event.type.default = Evento

journal.window.content.import.attendees = Partecipanti: { $attendees }

journal.window.content.import.book.by = ## { $book } di { $author }

journal.window.content.import.book.unknown = Libro sconosciuto

journal.window.content.import.duration.min = { $minutes } min

journal.window.content.import.event.untitled = Evento senza titolo

journal.window.content.import.links = Link: { $links }

journal.window.content.import.loc = Posizione { $location }

journal.window.content.import.location = Posizione: { $location }

journal.window.content.import.note.line = Nota: { $content }

journal.window.content.import.note.untitled = Nota senza titolo

journal.window.content.import.page = Pagina { $page }

journal.window.content.import.tags = Tag: { $tags }

journal.window.content.objects.class.bird = uccello

journal.window.content.objects.class.bottle = bottiglia

journal.window.content.objects.class.bowl = ciotola

journal.window.content.objects.class.car = auto

journal.window.content.objects.class.cell.phone = cellulare

journal.window.content.objects.class.cup = tazza

journal.window.content.objects.class.laptop = portatile

journal.window.content.objects.class.person = persona

journal.window.content.objects.class.tie = cravatta

journal.window.content.objects.class.truck = camion

journal.window.content.objects.class.tv = televisore

journal.window.content.objects.count = { $name } ×{ $count }

journal.window.content.objects.line = **Oggetti:** { $summary }

journal.window.content.observation.observed = - { $content } (osservato: { $day })

journal.window.content.possible.name = possibile nome: { $name }

journal.window.content.protected.video.body = Una finestra di { $service } era sullo schermo per circa { $duration } mentre l'ingresso (schermo e audio) era in pausa.

journal.window.content.protected.video.body.app = Una finestra di { $service } era sullo schermo in { $app } per circa { $duration } mentre l'ingresso (schermo e audio) era in pausa.

journal.window.content.protected.video.body.app.title = Una finestra di { $service } era sullo schermo in { $app }, con titolo "{ $title }", per circa { $duration } mentre l'ingresso (schermo e audio) era in pausa.

journal.window.content.protected.video.body.title = Una finestra di { $service } era sullo schermo, con titolo "{ $title }", per circa { $duration } mentre l'ingresso (schermo e audio) era in pausa.

journal.window.content.protected.video.heading = ## Video protetto: { $service }

journal.window.content.protected.video.minutes = { $quantity ->
    [one] 1 minuto
    [many] { $quantity } di minuti
   *[other] { $quantity } minuti
}

journal.window.content.raw.screen.category = **Categoria:** { $category }

journal.window.content.raw.screen.meeting = **Riunione** ({ $platform })

journal.window.content.raw.screen.participants = **Partecipanti:**

journal.window.content.raw.screen.share = **Condivisione dello schermo:**

journal.window.content.raw.screen.share.by = **Condivisione dello schermo di { $presenter }:**

journal.window.content.raw.screen.unknown = sconosciuto

journal.window.content.raw.screen.unknown.title = Sconosciuto

journal.window.content.screen.entities.heading = ## Entità

journal.window.content.screen.entities.none = Non specificato

journal.window.content.screen.entity.default = Entità

journal.window.content.workout.calories = { $kcal } kcal

journal.window.content.workout.distance = { $km } km

journal.window.content.workout.duration.h = { $hours } h

journal.window.content.workout.duration.hm = { $hours } h { $minutes } min

journal.window.content.workout.duration.m = { $minutes } min

journal.window.content.workout.elapsed = { $duration } trascorso

journal.window.content.workout.elevation = dislivello positivo { $m } m

journal.window.content.workout.flag.commute = casa-lavoro

journal.window.content.workout.flag.manual = inserito a mano

journal.window.content.workout.heading = ## Allenamento Strava: { $name }

journal.window.content.workout.hr.avg = frequenza cardiaca media { $avg } bpm

journal.window.content.workout.hr.avg.max = frequenza cardiaca media { $avg } bpm, massima { $max } bpm

journal.window.content.workout.hr.max = frequenza cardiaca massima { $max } bpm

journal.window.content.workout.more = altro da Strava:

journal.window.content.workout.moving = { $duration } in movimento

journal.window.content.workout.name.default = allenamento

journal.window.content.workout.power.avg = potenza media { $avg } W

journal.window.content.workout.power.avg.weighted = potenza media { $avg } W, ponderata { $weighted } W

journal.window.content.workout.power.weighted = potenza ponderata { $weighted } W

journal.window.content.workout.recorded = come risulta su Strava: { $details }

journal.window.content.workout.started = iniziato { $time }

journal.window.home.accessible.name.days.of.the.week = giorni della settimana

journal.window.home.accessible.name.originals.pages = pagine degli originali

journal.window.home.accessible.name.which.day = quale giorno

journal.window.home.account.tab.today = oggi

journal.window.home.account.tab.yesterday = ieri

journal.window.home.action.all.of.todays.activities = tutte le attività di oggi →

journal.window.home.action.check.status = controlla lo stato

journal.window.home.action.copy = copia

journal.window.home.action.how.this.was.written = come è stato scritto →

journal.window.home.action.open.it = aprilo →

journal.window.home.action.open.your.week = apri la tua settimana →

journal.window.home.action.see.health = vedi lo stato →

journal.window.home.action.see.the.run = vedi l'esecuzione →

journal.window.home.action.undo = annulla

journal.window.home.action.what.it.fixes = che cosa corregge ↗

journal.window.home.activity.source.browser = dal tuo browser

journal.window.home.activity.source.browser.named = dal tuo browser ({ $device })

journal.window.home.activity.source.computer = dal tuo computer

journal.window.home.activity.source.computer.named = dal tuo computer ({ $device })

journal.window.home.activity.source.ipad = dal tuo ipad

journal.window.home.activity.source.ipad.named = dal tuo ipad ({ $device })

journal.window.home.activity.source.iphone = dal tuo iphone

journal.window.home.activity.source.iphone.named = dal tuo iphone ({ $device })

journal.window.home.activity.source.mac = dal tuo mac

journal.window.home.activity.source.mac.named = dal tuo mac ({ $device })

journal.window.home.activity.source.phone = dal tuo telefono

journal.window.home.activity.source.phone.named = dal tuo telefono ({ $device })

journal.window.home.activity.source.phrase = dal tuo{ $slot1 }{ $kind }

journal.window.home.activity.source.phrase.named = dal tuo{ $slot1 }{ $kind }{ $slot2 }({ $device })

journal.window.home.activity.source.tablet = dal tuo tablet

journal.window.home.activity.source.tablet.named = dal tuo tablet ({ $device })

journal.window.home.activity.source.terminal = dal tuo terminale

journal.window.home.activity.source.terminal.named = dal tuo terminale ({ $device })

journal.window.home.activity.source.watch = dal tuo orologio

journal.window.home.activity.source.watch.named = dal tuo orologio ({ $device })

journal.window.home.activity.title.fallback = attività senza titolo

journal.window.home.attention.agent.errors.count = { $quantity ->
    [one] 1 errore di un agente oggi. chiedi che cosa è successo
    [many] { $count } di errori di agenti oggi. chiedi che cosa è successo
   *[other] { $count } errori di agenti oggi. chiedi che cosa è successo
}

journal.window.home.attention.heading = in attesa di te

journal.window.home.attention.import.complete = importazione completata.

journal.window.home.body.account.mindfulness = consapevolezza

journal.window.home.body.account.sleep = sonno

journal.window.home.body.activity.line = { $arrangement ->
       *[untimed-no-distance-no-source-intact] { $activity }.
        [untimed-no-distance-no-source-part-removed] { $activity }, una parte è stata rimossa.
        [untimed-no-distance-logged-intact] { $activity }, che hai annotato.
        [untimed-no-distance-logged-part-removed] { $activity }, che hai annotato, una parte è stata rimossa.
        [untimed-no-distance-recorded-intact] { $activity }, da { $source_names }.
        [untimed-no-distance-recorded-part-removed] { $activity }, da { $source_names }, una parte è stata rimossa.
        [untimed-distance-no-source-intact] { $activity }, { $distance }.
        [untimed-distance-no-source-part-removed] { $activity }, { $distance }, una parte è stata rimossa.
        [untimed-distance-logged-intact] { $activity }, { $distance }, che hai annotato.
        [untimed-distance-logged-part-removed] { $activity }, { $distance }, che hai annotato, una parte è stata rimossa.
        [untimed-distance-recorded-intact] { $activity }, { $distance }, da { $source_names }.
        [untimed-distance-recorded-part-removed] { $activity }, { $distance }, da { $source_names }, una parte è stata rimossa.
        [start-no-distance-no-source-intact] { $activity }, da { $started_at }.
        [start-no-distance-no-source-part-removed] { $activity }, da { $started_at }, una parte è stata rimossa.
        [start-no-distance-logged-intact] { $activity }, da { $started_at }, che hai annotato.
        [start-no-distance-logged-part-removed] { $activity }, da { $started_at }, che hai annotato, una parte è stata rimossa.
        [start-no-distance-recorded-intact] { $activity }, da { $started_at }, da { $source_names }.
        [start-no-distance-recorded-part-removed] { $activity }, da { $started_at }, da { $source_names }, una parte è stata rimossa.
        [start-distance-no-source-intact] { $activity }, da { $started_at }, { $distance }.
        [start-distance-no-source-part-removed] { $activity }, da { $started_at }, { $distance }, una parte è stata rimossa.
        [start-distance-logged-intact] { $activity }, da { $started_at }, { $distance }, che hai annotato.
        [start-distance-logged-part-removed] { $activity }, da { $started_at }, { $distance }, che hai annotato, una parte è stata rimossa.
        [start-distance-recorded-intact] { $activity }, da { $started_at }, { $distance }, da { $source_names }.
        [start-distance-recorded-part-removed] { $activity }, da { $started_at }, { $distance }, da { $source_names }, una parte è stata rimossa.
        [range-no-distance-no-source-intact] { $activity }, da { $started_at } a { $ended_at }.
        [range-no-distance-no-source-part-removed] { $activity }, da { $started_at } a { $ended_at }, una parte è stata rimossa.
        [range-no-distance-logged-intact] { $activity }, da { $started_at } a { $ended_at }, che hai annotato.
        [range-no-distance-logged-part-removed] { $activity }, da { $started_at } a { $ended_at }, che hai annotato, una parte è stata rimossa.
        [range-no-distance-recorded-intact] { $activity }, da { $started_at } a { $ended_at }, da { $source_names }.
        [range-no-distance-recorded-part-removed] { $activity }, da { $started_at } a { $ended_at }, da { $source_names }, una parte è stata rimossa.
        [range-distance-no-source-intact] { $activity }, da { $started_at } a { $ended_at }, { $distance }.
        [range-distance-no-source-part-removed] { $activity }, da { $started_at } a { $ended_at }, { $distance }, una parte è stata rimossa.
        [range-distance-logged-intact] { $activity }, da { $started_at } a { $ended_at }, { $distance }, che hai annotato.
        [range-distance-logged-part-removed] { $activity }, da { $started_at } a { $ended_at }, { $distance }, che hai annotato, una parte è stata rimossa.
        [range-distance-recorded-intact] { $activity }, da { $started_at } a { $ended_at }, { $distance }, da { $source_names }.
        [range-distance-recorded-part-removed] { $activity }, da { $started_at } a { $ended_at }, { $distance }, da { $source_names }, una parte è stata rimossa.
    }

journal.window.home.body.sources.and = { $start }{ $slot1 }e{ $slot2 }{ $last }

journal.window.home.body.time.with.day = { $time }{ $slot1 }di{ $slot2 }{ $day }

journal.window.home.briefing.expected.time = ieri di solito è scritto entro le 10 del mattino.

journal.window.home.briefing.late.hour.count = { $quantity ->
    [one] è in ritardo di 1 h.
    [many] è in ritardo di { $hours } h.
   *[other] è in ritardo di { $hours } h.
}

journal.window.home.briefing.written.timestamp = scritto alle { $written_at }

journal.window.home.button.label.show.all = mostra tutto →

journal.window.home.button.label.try.again = Riprova

journal.window.home.connections.events.count = { $quantity ->
    [one] 1 evento
    [many] { $count } di eventi
   *[other] { $count } eventi
}

journal.window.home.connections.evidence.transcription = trascritto come "{ $label }"

journal.window.home.connections.horizon.day = i collegamenti dal tempo nello stesso luogo risalgono solo al { $day }.

journal.window.home.connections.horizon.earlier.days = { $quantity ->
    [one] un giorno precedente è ancora nel tuo diario; quei collegamenti non sono ancora stati costruiti.
    [many] i { $earlier_days } di giorni precedenti sono ancora nel tuo diario; quei collegamenti non sono ancora stati costruiti.
   *[other] i { $earlier_days } giorni precedenti sono ancora nel tuo diario; quei collegamenti non sono ancora stati costruiti.
}

journal.window.home.connections.mentions.count = { $quantity ->
    [one] 1 nome citato nel tuo diario
    [many] { $count } di nomi citati nel tuo diario
   *[other] { $count } nomi citati nel tuo diario
}

journal.window.home.connections.moments.count = { $quantity ->
    [one] 1 momento
    [many] { $count } di momenti
   *[other] { $count } momenti
}

journal.window.home.connections.moments.recent.count = { $quantity ->
    [one] 1 momento · più di recente { $day }
    [many] { $count } di momenti · più di recente { $day }
   *[other] { $count } momenti · più di recente { $day }
}

journal.window.home.device.last.added.day = { $quantity ->
    [one] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } giorno fa.
    [many] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } di giorni fa.
   *[other] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } giorni fa.
}

journal.window.home.device.last.added.hour = { $quantity ->
    [one] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } ora fa.
    [many] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } di ore fa.
   *[other] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } ore fa.
}

journal.window.home.device.last.added.minute = { $quantity ->
    [one] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } minuto fa.
    [many] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } di minuti fa.
   *[other] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } minuti fa.
}

journal.window.home.device.last.added.month = { $quantity ->
    [one] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } mese fa.
    [many] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } di mesi fa.
   *[other] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } mesi fa.
}

journal.window.home.device.last.added.second = { $quantity ->
    [one] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } secondo fa.
    [many] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } di secondi fa.
   *[other] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } secondi fa.
}

journal.window.home.device.last.added.week = { $quantity ->
    [one] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } settimana fa.
    [many] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } di settimane fa.
   *[other] l'app di solstone su { $name } ha aggiunto l'ultima volta al tuo diario { $count } settimane fa.
}

journal.window.home.device.paired = l'app di solstone su{ $slot1 }{ $name }{ $slot2 }è accoppiata.

journal.window.home.device.paired.awaiting = l'app di solstone su{ $slot1 }{ $name }{ $slot2 }è accoppiata. ciò che condividi con lei arriva nel tuo diario circa ogni 5 minuti.

journal.window.home.disclosure.label.review.originals = rivedi gli originali

journal.window.home.disclosure.label.what.this.summary.covers = che cosa copre questo riepilogo

journal.window.home.download.speech.progress = scaricamento del modello del parlato · { $downloaded_gb } GB di { $total_gb } GB

journal.window.home.download.thinking.progress = scaricamento del modello in locale · { $downloaded_gb } GB di { $total_gb } GB

journal.window.home.first.segment.arrived = il tuo primo segmento, da{ $slot1 }{ $time }, è arrivato.

journal.window.home.first.segment.pending = il tuo primo segmento, da{ $slot1 }{ $time }, è arrivato. il ragionamento non è ancora in pari.

journal.window.home.first.segment.read = il tuo primo segmento, da{ $slot1 }{ $time }, è arrivato, e il ragionamento l'ha letto.

journal.window.home.health.age.just.now = proprio ora

journal.window.home.health.attention.count = { $quantity ->
    [one] 1 cosa richiede la tua attenzione
    [many] { $count } di cose richiedono la tua attenzione
   *[other] { $count } cose richiedono la tua attenzione
}

journal.window.home.health.awaiting.first.headline = l'app di solstone su uno dei tuoi dispositivi non ha ancora aggiunto niente al tuo diario.

journal.window.home.health.backlog.day.needs.help = un giorno del diario ha bisogno di una mano.

journal.window.home.health.device.age = { $device } ({ $age })

journal.window.home.health.device.setup = configurane uno →

journal.window.home.health.device.sources = { $device } ({ $sources })

journal.window.home.health.devices.view = vedi i dispositivi →

journal.window.home.health.empty.registry.headline = nessun dispositivo esegue ancora l'app di solstone. configurane uno per avviare il tuo diario.

journal.window.home.health.no.eligible.headline = nessuno dei tuoi dispositivi ha l'app di solstone configurata per aggiungere al tuo diario in questo momento.

journal.window.home.health.not.yet.active.headline = l'app di solstone sta aggiungendo al tuo diario.

journal.window.home.health.quiet.devices = { $quantity ->
    [one] 1 dispositivo non ha aggiunto al tuo diario di recente: { $devices }.
    [many] { $count } di dispositivi non hanno aggiunto al tuo diario di recente: { $devices }.
   *[other] { $count } dispositivi non hanno aggiunto al tuo diario di recente: { $devices }.
}

journal.window.home.health.quiet.elapsed.day = { $quantity ->
    [one] { $device } non ha aggiunto al tuo diario da { $count } giorno.
    [many] { $device } non ha aggiunto al tuo diario da { $count } di giorni.
   *[other] { $device } non ha aggiunto al tuo diario da { $count } giorni.
}

journal.window.home.health.quiet.elapsed.hour = { $quantity ->
    [one] { $device } non ha aggiunto al tuo diario da { $count } ora.
    [many] { $device } non ha aggiunto al tuo diario da { $count } di ore.
   *[other] { $device } non ha aggiunto al tuo diario da { $count } ore.
}

journal.window.home.health.quiet.elapsed.just.now = { $device } ha aggiunto al tuo diario l'ultima volta proprio ora.

journal.window.home.health.quiet.elapsed.minute = { $quantity ->
    [one] { $device } non ha aggiunto al tuo diario da { $count } minuto.
    [many] { $device } non ha aggiunto al tuo diario da { $count } di minuti.
   *[other] { $device } non ha aggiunto al tuo diario da { $count } minuti.
}

journal.window.home.health.quiet.elapsed.month = { $quantity ->
    [one] { $device } non ha aggiunto al tuo diario da { $count } mese.
    [many] { $device } non ha aggiunto al tuo diario da { $count } di mesi.
   *[other] { $device } non ha aggiunto al tuo diario da { $count } mesi.
}

journal.window.home.health.quiet.elapsed.second = { $quantity ->
    [one] { $device } non ha aggiunto al tuo diario da { $count } secondo.
    [many] { $device } non ha aggiunto al tuo diario da { $count } di secondi.
   *[other] { $device } non ha aggiunto al tuo diario da { $count } secondi.
}

journal.window.home.health.quiet.elapsed.week = { $quantity ->
    [one] { $device } non ha aggiunto al tuo diario da { $count } settimana.
    [many] { $device } non ha aggiunto al tuo diario da { $count } di settimane.
   *[other] { $device } non ha aggiunto al tuo diario da { $count } settimane.
}

journal.window.home.health.quiet.one.unknown = { $device }{ $slot1 }non ha aggiunto al tuo diario di recente.

journal.window.home.health.ready = tutto funziona

journal.window.home.health.residue.headline = niente richiede la tua attenzione in questo momento.

journal.window.home.health.unavailable.headline = lo stato dei tuoi dispositivi non è chiaro in questo momento.

journal.window.home.health.unnamed.idle.issue = l'app di solstone su uno dei tuoi dispositivi non ha aggiunto niente al tuo diario di recente.

journal.window.home.health.upload.devices = { $quantity ->
    [one] non è stato possibile accettare nel tuo diario i caricamenti da { $devices }.
    [many] non è stato possibile accettare nel tuo diario i caricamenti da { $count } di dispositivi: { $devices }.
   *[other] non è stato possibile accettare nel tuo diario i caricamenti da { $count } dispositivi: { $devices }.
}

journal.window.home.held.activity.safety.check = l'elaborazione dell'attività è trattenuta per un controllo di sicurezza.

journal.window.home.held.all.safety.check = tutto il lavoro in coda è trattenuto per un controllo di sicurezza.

journal.window.home.held.daily.safety.check = l'elaborazione giornaliera è trattenuta per un controllo di sicurezza.

journal.window.home.held.flush.safety.check = il completamento delle attività è trattenuto per un controllo di sicurezza.

journal.window.home.held.queued.safety.check = l'elaborazione in coda è trattenuta per un controllo di sicurezza.

journal.window.home.held.search.safety.check = l'aggiornamento della ricerca è trattenuto per un controllo di sicurezza.

journal.window.home.held.segment.safety.check = l'elaborazione del segmento è trattenuta per un controllo di sicurezza.

journal.window.home.held.weekly.safety.check = la riflessione settimanale è trattenuta per un controllo di sicurezza.

journal.window.home.link.label.all.connections = tutti i collegamenti →

journal.window.home.link.label.cancel.on.the.thinking.page = annulla nella pagina di ragionamento →

journal.window.home.link.label.check.system.health = controlla lo stato del sistema →

journal.window.home.link.label.health = stato →

journal.window.home.link.label.name.voices.in.speakers = nomina le voci nei parlanti →

journal.window.home.link.label.other.agents.and.pairing.codes = altri agenti e codici di accoppiamento →

journal.window.home.link.label.pair.a.device = accoppia un dispositivo →

journal.window.home.link.label.try.again.on.the.thinking.page = riprova nella pagina di ragionamento →

journal.window.home.narrative.coverage.batch.counts = { $segment_quantity ->
    [one] in base a { $processed_segments } di { $batch_segments } segmento elaborato e { $processed_activities } di { $batch_activities } { $activity_quantity ->
        [one] elemento di attività
        [many] elementi di attività
       *[other] elementi di attività
    } in questo lotto.
    [many] in base a { $processed_segments } di { $batch_segments } di segmenti elaborati e { $processed_activities } di { $batch_activities } { $activity_quantity ->
        [one] elemento di attività
        [many] elementi di attività
       *[other] elementi di attività
    } in questo lotto.
   *[other] in base a { $processed_segments } di { $batch_segments } segmenti elaborati e { $processed_activities } di { $batch_activities } { $activity_quantity ->
        [one] elemento di attività
        [many] elementi di attività
       *[other] elementi di attività
    } in questo lotto.
}

journal.window.home.narrative.coverage.clock = questo è il tempo di elaborazione, non quando è avvenuta l'attività.

journal.window.home.narrative.coverage.overlap = gli elementi possono sovrapporsi; questo non è un riepilogo dell'intera giornata.

journal.window.home.narrative.coverage.started = la finestra di elaborazione inizia a { $start_time }.

journal.window.home.narrative.updated.timestamp = aggiornato a { $updated_at }

journal.window.home.needs.device.one = { $name }{ $slot1 }non si connette al tuo diario.

journal.window.home.needs.devices.named = { $quantity ->
    [one] { $count } dispositivo non si connette al tuo diario: { $names }.
    [many] { $count } di dispositivi non si connettono al tuo diario: { $names }.
   *[other] { $count } dispositivi non si connettono al tuo diario: { $names }.
}

journal.window.home.needs.devices.unnamed = { $quantity ->
    [one] { $count } dispositivo non si connette al tuo diario.
    [many] { $count } di dispositivi non si connettono al tuo diario.
   *[other] { $count } dispositivi non si connettono al tuo diario.
}

journal.window.home.needs.devices.unnamed.count = { $quantity ->
    [one] { $count } dispositivo senza nome
    [many] { $count } di dispositivi senza nome
   *[other] { $count } dispositivi senza nome
}

journal.window.home.needs.sources.failing = l'app di solstone su uno dei tuoi dispositivi ha difficoltà ad aggiungere{ $slot1 }{ $sources }{ $slot2 }al tuo diario.

journal.window.home.needs.voice.candidate.ready = il tuo diario potrebbe aver trovato la tua voce. riproduci qualche campione e di' se sei tu.

journal.window.home.needs.voice.damaged = la tua voce salvata non si può leggere. rimuovila nella pagina dei parlanti, poi insegnala di nuovo.

journal.window.home.needs.voice.declined.template = la tua voce non è ancora configurata. il tuo diario la cerca di nuovo dopo{ $slot1 }{ $date }, oppure puoi insegnarla ora.

journal.window.home.needs.voice.needs.more.audio = la tua voce non è ancora configurata. il tuo diario la cerca ogni giorno, oppure puoi insegnarla ora.

journal.window.home.needs.voice.next.look.date = { $weekday }{ $slot1 }{ $day }{ $slot2 }{ $month }

journal.window.home.needs.voice.too.short.or.mixed = la tua voce non è ancora configurata. finora le tue frasi sono per lo più brevi o mescolate con altre voci, quindi insegnarla aiuta di più.

journal.window.home.needs.voice.unusable = la tua voce non si può configurare da ciò che c'è. insegnala nella pagina dei parlanti.

journal.window.home.night.briefing.not.written = ieri non è stato redatto.

journal.window.home.night.gap.additional = { $quantity ->
    [one] …e { $count } altro non ha finito.
    [many] …e { $count } di altri non hanno finito.
   *[other] …e { $count } altri non hanno finito.
}

journal.window.home.night.gap.runs.failed = { $quantity ->
    [one] l'esecuzione di { $label } non è finita.
    [many] { $count } di esecuzioni di { $label } non sono finite.
   *[other] { $count } esecuzioni di { $label } non sono finite.
}

journal.window.home.night.gap.runs.failed.briefing = { $quantity ->
    [one] l'esecuzione del riepilogo del giorno non è finita.
    [many] { $count } di esecuzioni del riepilogo del giorno non sono finite.
   *[other] { $count } esecuzioni del riepilogo del giorno non sono finite.
}

journal.window.home.night.gap.runs.failed.documents = { $quantity ->
    [one] l'esecuzione del documento non è finita.
    [many] { $count } di esecuzioni del documento non sono finite.
   *[other] { $count } esecuzioni del documento non sono finite.
}

journal.window.home.night.gap.runs.lost = { $quantity ->
    [one] non è stato possibile avviare l'esecuzione di { $label }.
    [many] non è stato possibile avviare { $count } di esecuzioni di { $label }.
   *[other] non è stato possibile avviare { $count } esecuzioni di { $label }.
}

journal.window.home.night.gap.runs.lost.briefing = { $quantity ->
    [one] non è stato possibile avviare l'esecuzione del riepilogo del giorno.
    [many] non è stato possibile avviare { $count } di esecuzioni del riepilogo del giorno.
   *[other] non è stato possibile avviare { $count } esecuzioni del riepilogo del giorno.
}

journal.window.home.night.gap.runs.lost.documents = { $quantity ->
    [one] non è stato possibile avviare l'esecuzione del documento.
    [many] non è stato possibile avviare { $count } di esecuzioni del documento.
   *[other] non è stato possibile avviare { $count } esecuzioni del documento.
}

journal.window.home.night.notes.not.written = non tutte le note di ieri sono state redatte.

journal.window.home.night.processing.did.not.finish = parte dell'elaborazione notturna non è finita.

journal.window.home.night.review.did.not.finish = la revisione notturna non è finita.

journal.window.home.night.status.unavailable = lo stato di elaborazione di ieri non è disponibile.

journal.window.home.night.work.did.not.finish = parte del lavoro notturno non è finito.

journal.window.home.originals.bulk.limit = scegli fino a { $selection_limit } elementi alla volta. non è stato eliminato niente.

journal.window.home.originals.bulk.selection.count = { $quantity ->
    [one] 1 selezionato
    [many] { $selected_count } di selezionati
   *[other] { $selected_count } selezionati
}

journal.window.home.originals.bulk.selection.over.cap = { $selected_count } selezionati. un'azione copre fino a { $selection_limit }.

journal.window.home.originals.card.page.total = { $quantity ->
    [one] in questa pagina: 1 originale · { $size }
    [many] in questa pagina: { $original_count } di originali · { $size }
   *[other] in questa pagina: { $original_count } originali · { $size }
}

journal.window.home.originals.confirm.bulk.delete.first = questo elimina { $original_count } originali.

journal.window.home.originals.confirm.bulk.delete.third = nient'altro va con loro.

journal.window.home.originals.confirm.bulk.trash.third = restano nel cestino, poi il cestino li cancella e il loro spazio torna.

journal.window.home.originals.confirm.bulk.trash.until.erase.first = questo sposta { $original_count } originali nel cestino.

journal.window.home.originals.confirm.bulk.trash.until.erase.fourth = puoi ripristinarli fino ad allora.

journal.window.home.originals.confirm.bulk.trash.until.erase.third = { $days_quantity ->
    [one] il cestino li cancella dopo 1 giorno, e il loro spazio torna allora.
    [many] il cestino li cancella dopo { $retention_days } di giorni, e il loro spazio torna allora.
   *[other] il cestino li cancella dopo { $retention_days } giorni, e il loro spazio torna allora.
}

journal.window.home.originals.confirm.delete.action = { $quantity ->
    [one] eliminalo
    [many] eliminali
   *[other] eliminali
}

journal.window.home.originals.confirm.delete.default.stream.first = { $quantity ->
    [one] questo elimina 1 originale da { $date_and_time }.
    [many] questo elimina { $original_count } di originali da { $date_and_time }.
   *[other] questo elimina { $original_count } originali da { $date_and_time }.
}

journal.window.home.originals.confirm.delete.heading = { $quantity ->
    [one] elimina questo originale?
    [many] elimina questi originali?
   *[other] elimina questi originali?
}

journal.window.home.originals.confirm.delete.named.stream.first = { $quantity ->
    [one] questo elimina 1 originale da { $date_and_time } · { $stream_name }.
    [many] questo elimina { $original_count } di originali da { $date_and_time } · { $stream_name }.
   *[other] questo elimina { $original_count } originali da { $date_and_time } · { $stream_name }.
}

journal.window.home.originals.confirm.delete.named.stream.fourth = non si può annullare.

journal.window.home.originals.confirm.delete.named.stream.second = il tuo backup ha una copia che da qui non si può controllare.

journal.window.home.originals.confirm.delete.named.stream.third = { $quantity ->
    [one] nient'altro va con questo.
    [many] nient'altro va con questi.
   *[other] nient'altro va con questi.
}

journal.window.home.originals.confirm.mixed.delete.first = { $quantity ->
    [one] quello che viene eliminato è stato copiato fuori dal tuo dispositivo dal tuo backup.
    [many] quelli che vengono eliminati sono stati copiati fuori dal tuo dispositivo dal tuo backup.
   *[other] quelli che vengono eliminati sono stati copiati fuori dal tuo dispositivo dal tuo backup.
}

journal.window.home.originals.confirm.mixed.delete.third = { $quantity ->
    [one] eliminarlo non si può annullare.
    [many] eliminarli non si può annullare.
   *[other] eliminarli non si può annullare.
}

journal.window.home.originals.confirm.mixed.summary = questo elimina { $deleted_count } di questi originali e ne sposta { $trashed_count } nel cestino. nient'altro va con loro.

journal.window.home.originals.confirm.mixed.trash.first = { $quantity ->
    [one] quello che viene spostato nel cestino resta lì, poi il cestino lo cancella e il suo spazio torna.
    [many] quelli che vengono spostati nel cestino restano lì, poi il cestino li cancella e il loro spazio torna.
   *[other] quelli che vengono spostati nel cestino restano lì, poi il cestino li cancella e il loro spazio torna.
}

journal.window.home.originals.confirm.mixed.trash.until.erase.first = { $quantity ->
    [one] quello che viene spostato nel cestino viene cancellato { $days_quantity ->
        [one] dopo 1 giorno
        [many] dopo { $retention_days } di giorni
       *[other] dopo { $retention_days } giorni
    }, e il suo spazio torna allora.
    [many] quelli che vengono spostati nel cestino vengono cancellati { $days_quantity ->
        [one] dopo 1 giorno
        [many] dopo { $retention_days } di giorni
       *[other] dopo { $retention_days } giorni
    }, e il loro spazio torna allora.
   *[other] quelli che vengono spostati nel cestino vengono cancellati { $days_quantity ->
        [one] dopo 1 giorno
        [many] dopo { $retention_days } di giorni
       *[other] dopo { $retention_days } giorni
    }, e il loro spazio torna allora.
}

journal.window.home.originals.confirm.trash.named.stream.third = { $quantity ->
    [one] resta nel cestino, poi il cestino lo cancella e il suo spazio torna.
    [many] restano nel cestino, poi il cestino li cancella e il loro spazio torna.
   *[other] restano nel cestino, poi il cestino li cancella e il loro spazio torna.
}

journal.window.home.originals.confirm.trash.until.erase.default.stream.first = { $quantity ->
    [one] questo sposta 1 originale da { $date_and_time } nel cestino.
    [many] questo sposta { $original_count } di originali da { $date_and_time } nel cestino.
   *[other] questo sposta { $original_count } originali da { $date_and_time } nel cestino.
}

journal.window.home.originals.confirm.trash.until.erase.named.stream.first = { $quantity ->
    [one] questo sposta 1 originale da { $date_and_time } · { $stream_name } nel cestino.
    [many] questo sposta { $original_count } di originali da { $date_and_time } · { $stream_name } nel cestino.
   *[other] questo sposta { $original_count } originali da { $date_and_time } · { $stream_name } nel cestino.
}

journal.window.home.originals.confirm.trash.until.erase.named.stream.fourth = { $quantity ->
    [one] puoi ripristinarlo fino ad allora.
    [many] puoi ripristinarli fino ad allora.
   *[other] puoi ripristinarli fino ad allora.
}

journal.window.home.originals.confirm.trash.until.erase.named.stream.third = { $quantity ->
    [one] il cestino lo cancella { $days_quantity ->
        [one] dopo 1 giorno
        [many] dopo { $retention_days } di giorni
       *[other] dopo { $retention_days } giorni
    }, e il suo spazio torna allora.
    [many] il cestino li cancella { $days_quantity ->
        [one] dopo 1 giorno
        [many] dopo { $retention_days } di giorni
       *[other] dopo { $retention_days } giorni
    }, e il loro spazio torna allora.
   *[other] il cestino li cancella { $days_quantity ->
        [one] dopo 1 giorno
        [many] dopo { $retention_days } di giorni
       *[other] dopo { $retention_days } giorni
    }, e il loro spazio torna allora.
}

journal.window.home.originals.copy.bulk.clear = azzera la selezione

journal.window.home.originals.copy.bulk.delete = elimina i selezionati

journal.window.home.originals.copy.bulk.keep = tieni i selezionati

journal.window.home.originals.copy.bulk.select.all = seleziona questa pagina

journal.window.home.originals.copy.card.empty = non c'è niente che ti aspetti.

journal.window.home.originals.copy.card.heading = originali in attesa di te

journal.window.home.originals.copy.card.loading = caricamento degli originali…

journal.window.home.originals.copy.card.subhead = l'audio e il video originali segnati per l'eliminazione, più ciò che un'eliminazione ha lasciato incompiuto. niente di segnato qui viene eliminato finché non lo dici.

journal.window.home.originals.copy.card.support = contatta il supporto per un aiuto

journal.window.home.originals.copy.card.too.large = i dettagli di eliminazione di questo elemento sono troppo grandi per rivederli qui.

journal.window.home.originals.copy.card.unavailable = non è stato possibile caricare l'elenco.

journal.window.home.originals.copy.confirm.cancel = annulla

journal.window.home.originals.copy.confirm.go.trash = sposta nel cestino

journal.window.home.originals.copy.confirm.recover.body = questo completa ogni eliminazione che si era fermata a metà. ogni originale non ancora eliminato torna nel suo segmento. il resto resta eliminato, e quelli segnati dalle tue impostazioni di conservazione sono nel cestino.

journal.window.home.originals.copy.confirm.recover.go = completali

journal.window.home.originals.copy.confirm.recover.heading = completa le eliminazioni che si sono fermate?

journal.window.home.originals.copy.done.clause.halted = si è fermato lì.

journal.window.home.originals.copy.done.clause.nothing.deleted = non è stato eliminato niente.

journal.window.home.originals.copy.done.declined.failed = non è stato possibile toglierlo dall'elenco, quindi resta. non è stato eliminato niente.

journal.window.home.originals.copy.done.declined.unknown = non è stato eliminato niente. non è chiaro se è ancora nell'elenco.

journal.window.home.originals.copy.done.kept.offload = tenuto per ora. torna dopo il tuo prossimo backup.

journal.window.home.originals.copy.done.kept.policy = tenuto per ora. torna la prossima volta che questa parte dell'elenco si ricostruisce.

journal.window.home.originals.copy.done.open.trash = apri il cestino

journal.window.home.originals.copy.done.recover.failed = non è stato possibile completarli. ciò che resta è ancora lì.

journal.window.home.originals.copy.done.recover.unknown = il completamento si è fermato prima di poter confermare che cosa è successo. non è chiaro che cosa è stato completato.

journal.window.home.originals.copy.done.recovered = le eliminazioni che si erano fermate ora sono completate.

journal.window.home.originals.copy.done.recovered.leftover = alcune sono completate. almeno un'eliminazione che si era fermata è ancora in attesa.

journal.window.home.originals.copy.done.recovered.none = non restava niente da finire.

journal.window.home.originals.copy.done.unknown = l'eliminazione si è fermata prima di poter confermare che cosa è successo. non è chiaro che cosa è stato eliminato e che cosa no.

journal.window.home.originals.copy.failed.badge = non ha finito

journal.window.home.originals.copy.failed.body = un'eliminazione che hai avviato si è fermata a metà. completarla rimette ogni originale non ancora eliminato, e il resto resta eliminato.

journal.window.home.originals.copy.failed.finish = completa l'eliminazione

journal.window.home.originals.copy.operation.completed = eliminazione completata.

journal.window.home.originals.copy.operation.held = l'eliminazione non è del tutto finita.

journal.window.home.originals.copy.operation.record.unavailable = il registro dell'eliminazione non è disponibile.

journal.window.home.originals.copy.operation.refused = l'eliminazione non è partita.

journal.window.home.originals.copy.operation.running = l'eliminazione è in corso.

journal.window.home.originals.copy.operation.stop = interrompi l'eliminazione

journal.window.home.originals.copy.operation.stop.pending = richiesta di interruzione.

journal.window.home.originals.copy.operation.stopping = interruzione richiesta. in attesa che il lavoro finisca.

journal.window.home.originals.copy.operation.submitting = invio della tua richiesta di eliminazione.

journal.window.home.originals.copy.operation.unavailable = non è stato possibile controllare il risultato attuale.

journal.window.home.originals.copy.operation.unknown = non è chiaro che cosa è stato eliminato e che cosa no.

journal.window.home.originals.copy.page.selection = la selezione vale per questa pagina e si azzera quando cambi pagina.

journal.window.home.originals.copy.row.delete = elimina gli originali

journal.window.home.originals.copy.row.keep = tieni per ora

journal.window.home.originals.heading.count = { $quantity ->
    [one] originali · 1 elemento
    [many] originali · { $count } di elementi
   *[other] originali · { $count } elementi
}

journal.window.home.originals.page.item.range = elementi { $first_item }–{ $last_item } di { $total_items }

journal.window.home.originals.receipt.deleted = { $quantity ->
    [one] eliminato 1 originale.
    [many] eliminati { $original_count } di originali.
   *[other] eliminati { $original_count } originali.
}

journal.window.home.originals.receipt.kept = { $quantity ->
    [one] tenuto 1 per ora.
    [many] tenuti { $original_count } per ora.
   *[other] tenuti { $original_count } per ora.
}

journal.window.home.originals.receipt.moved.to.trash = { $quantity ->
    [one] spostato 1 originale nel cestino.
    [many] spostati { $original_count } di originali nel cestino.
   *[other] spostati { $original_count } originali nel cestino.
}

journal.window.home.originals.receipt.none.deleted = { $quantity ->
    [one] non è stato possibile eliminarlo. non è stato eliminato niente.
    [many] non è stato possibile eliminarli. non è stato eliminato niente.
   *[other] non è stato possibile eliminarli. non è stato eliminato niente.
}

journal.window.home.originals.receipt.not.kept = { $quantity ->
    [one] non è stato possibile togliere 1 altro dall'elenco, quindi resta.
    [many] non è stato possibile togliere { $remaining_count } di altri dall'elenco, quindi restano.
   *[other] non è stato possibile togliere { $remaining_count } altri dall'elenco, quindi restano.
}

journal.window.home.originals.receipt.not.removed = { $quantity ->
    [one] non è stato possibile eliminare 1 altro.
    [many] non è stato possibile eliminare { $remaining_count } di altri.
   *[other] non è stato possibile eliminare { $remaining_count } altri.
}

journal.window.home.originals.receipt.refused.named = { $original_name }: { $reason }

journal.window.home.originals.receipt.refused.reason = { $reason }

journal.window.home.originals.receipt.space.after.erase = { $quantity ->
    [one] viene cancellato { $erase_day }, e il suo spazio torna allora. puoi cancellarlo prima dal cestino.
    [many] vengono cancellati { $erase_day }, e il loro spazio torna allora. puoi cancellarli prima dal cestino.
   *[other] vengono cancellati { $erase_day }, e il loro spazio torna allora. puoi cancellarli prima dal cestino.
}

journal.window.home.originals.receipt.space.without.erase.day = { $quantity ->
    [one] il suo spazio torna quando viene cancellato dal cestino.
    [many] il loro spazio torna quando vengono cancellati dal cestino.
   *[other] il loro spazio torna quando vengono cancellati dal cestino.
}

journal.window.home.originals.review.unfinished = rivedi gli originali · le eliminazioni non finite vanno riviste

journal.window.home.originals.row.contents.count = { $quantity ->
    [one] 1 originale · { $size }
    [many] { $original_count } di originali · { $size }
   *[other] { $original_count } originali · { $size }
}

journal.window.home.originals.row.copied.by.backup = { $quantity ->
    [one] 1 originale copiato fuori dal tuo dispositivo dal tuo backup.
    [many] { $original_count } di originali copiati fuori dal tuo dispositivo dal tuo backup.
   *[other] { $original_count } originali copiati fuori dal tuo dispositivo dal tuo backup.
}

journal.window.home.originals.row.copied.by.backup.detail = { $quantity ->
    [one] non c'è modo di controllare che sia ancora lì.
    [many] non c'è modo di controllare che siano ancora lì.
   *[other] non c'è modo di controllare che siano ancora lì.
}

journal.window.home.originals.row.identity.stream = { $date_and_time } · { $stream_name }

journal.window.home.originals.row.marked.by.retention = { $quantity ->
    [one] 1 originale segnato dalle tue impostazioni di conservazione.
    [many] { $original_count } di originali segnati dalle tue impostazioni di conservazione.
   *[other] { $original_count } originali segnati dalle tue impostazioni di conservazione.
}

journal.window.home.originals.row.marked.by.retention.detail = questa parte dell'elenco si ricostruisce ogni giorno.

journal.window.home.originals.row.nothing.else.goes = { $quantity ->
    [one] nient'altro va con questo.
    [many] nient'altro va con questi.
   *[other] nient'altro va con questi.
}

journal.window.home.originals.selection.label = { $checkbox } seleziona questo elemento

journal.window.home.response.briefing.invalid = Risposta del riepilogo malformata

journal.window.home.response.pulse.failed = Non è stato possibile caricare il pulse

journal.window.home.response.pulse.invalid = Risposta del pulse malformata

journal.window.home.response.yesterday.failed = non è stato possibile caricare ieri.

journal.window.home.section.heading.connections = connessioni

journal.window.home.section.heading.getting.started = per iniziare

journal.window.home.section.heading.next = avanti

journal.window.home.section.heading.your.week = la tua settimana

journal.window.home.security.device = l'app di solstone su{ $slot1 }{ $device }{ $slot2 }non ha una correzione di sicurezza. aggiornala a{ $slot3 }{ $version }{ $slot4 }o oltre.

journal.window.home.security.journal = al tuo diario manca una correzione di sicurezza. aggiornalo a{ $slot1 }{ $version }{ $slot2 }o oltre.

journal.window.home.security.unnamed.device = l'app di solstone su uno dei tuoi dispositivi non ha una correzione di sicurezza. aggiornala a { $version } o oltre.

journal.window.home.setup.action.choose = scegli →

journal.window.home.setup.action.finish = completa →

journal.window.home.setup.agent.choose.connector = in{ $slot1 }{ $agent }, scegli

journal.window.home.setup.agent.description = lascia che{ $slot1 }{ $agent }{ $slot2 }o un altro agente legga il tuo diario.

journal.window.home.setup.phone.download = ottieni solstone sul tuo telefono su{ $slot1 }{ $site }, poi accoppialo al tuo diario.

journal.window.home.setup.speech.package.repair = ripara o reinstalla l'app del diario, poi riavvia il tuo diario.

journal.window.home.text.backup.status.is.unavailable = lo stato del backup non è disponibile.

journal.window.home.text.no.time.measured = nessun tempo misurato

journal.window.home.text.originals.the.list.couldn.t.be = originali · non è stato possibile caricare l'elenco.

journal.window.home.text.queued.work.status.is.unavailable = lo stato del lavoro in coda non è disponibile.

journal.window.home.text.service.status.is.unavailable = lo stato del servizio non è disponibile.

journal.window.home.text.the.backup.key.couldn.t.open = non è stato possibile aprire la destinazione del backup con la chiave di backup. controlla il backup nello stato.

journal.window.home.time.ago.day = { $quantity ->
    [one] { $count } giorno fa
    [many] { $count } di giorni fa
   *[other] { $count } giorni fa
}

journal.window.home.time.ago.hour = { $quantity ->
    [one] { $count } ora fa
    [many] { $count } di ore fa
   *[other] { $count } ore fa
}

journal.window.home.time.ago.minute = { $quantity ->
    [one] { $count } minuto fa
    [many] { $count } di minuti fa
   *[other] { $count } minuti fa
}

journal.window.home.time.ago.month = { $quantity ->
    [one] { $count } mese fa
    [many] { $count } di mesi fa
   *[other] { $count } mesi fa
}

journal.window.home.time.ago.second = { $quantity ->
    [one] { $count } secondo fa
    [many] { $count } di secondi fa
   *[other] { $count } secondi fa
}

journal.window.home.time.ago.week = { $quantity ->
    [one] { $count } settimana fa
    [many] { $count } di settimane fa
   *[other] { $count } settimane fa
}

journal.window.home.today.activity.count = { $quantity ->
    [one] 1 attività
    [many] { $activity_count } di attività
   *[other] { $activity_count } attività
}

journal.window.home.today.activity.preview.count = { $quantity ->
    [one] { $preview_count } di { $activity_count } attività
    [many] { $preview_count } di { $activity_count } di attività
   *[other] { $preview_count } di { $activity_count } attività
}

journal.window.home.today.duration.hour.and.minute.count = { $quantity ->
    [one] { $hours } h { $minute_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
    [many] { $hours } h { $minute_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
   *[other] { $hours } h { $minute_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
}

journal.window.home.today.duration.hour.count = { $quantity ->
    [one] { $hours } h
    [many] { $hours } h
   *[other] { $hours } h
}

journal.window.home.today.duration.minute.count = { $quantity ->
    [one] { $minutes } min
    [many] { $minutes } min
   *[other] { $minutes } min
}

journal.window.home.today.facets.measured.count = { $quantity ->
    [one] tempo in ogni facetta · misurato per { $measured_count } di 1 attività
    [many] tempo in ogni facetta · misurato per { $measured_count } di { $activity_count } di attività
   *[other] tempo in ogni facetta · misurato per { $measured_count } di { $activity_count } attività
}

journal.window.home.visible.text.add.your.phone = aggiungi il tuo telefono

journal.window.home.visible.text.and.authenticate = e autenticati.

journal.window.home.visible.text.choose.another.way.when.you.re = scegli un altro modo quando vuoi. il tuo diario tiene tutto fino ad allora.

journal.window.home.visible.text.connect.an.agent = connetti un agente

journal.window.home.visible.text.connections.are.unavailable.right.now = le connessioni non sono disponibili ora.

journal.window.home.visible.text.connections.found.in.your.journal = connessioni trovate nel tuo diario

journal.window.home.visible.text.couldn.t.be.written.out = non è stato possibile redigerlo.

journal.window.home.visible.text.downloading.the.local.model = scaricamento del modello in locale

journal.window.home.visible.text.downloading.the.speech.model = scaricamento del modello del parlato

journal.window.home.visible.text.from = da

journal.window.home.visible.text.from.your.devices.and.apps = dai tuoi dispositivi e dalle tue app

journal.window.home.visible.text.hasn.t.been.written.out.yet = non è stato ancora redatto.

journal.window.home.visible.text.it.appears.here.as.text = compare qui come testo.

journal.window.home.visible.text.leave.out.of.this.week = lascia fuori da questa settimana

journal.window.home.visible.text.left.out.of.this.week = lasciato fuori da questa settimana ·

journal.window.home.visible.text.loading.your.week = caricamento della tua settimana…

journal.window.home.visible.text.next = avanti

journal.window.home.visible.text.no.direct.connections.yet = ancora nessuna connessione diretta.

journal.window.home.visible.text.no.named.connections.yet = ancora nessuna connessione nominata.

journal.window.home.visible.text.no.recent.activity.yet.today = ancora nessuna attività recente oggi.

journal.window.home.visible.text.no.speech.was.found.in.the = non è stato trovato parlato nel segmento da

journal.window.home.visible.text.nothing.from.yesterday.to.write.up = niente di ieri da redigere.

journal.window.home.visible.text.nothing.from.yesterday.was.written.up = niente di ieri è stato redatto.

journal.window.home.visible.text.nothing.with.sound.has.reached.your = non è ancora arrivato niente con suono nel tuo diario. attiva il microfono nell'app di solstone per provare.

journal.window.home.visible.text.often.around.events.only = spesso intorno, solo eventi

journal.window.home.visible.text.on.your.calendar.only = solo sul tuo calendario

journal.window.home.visible.text.on.your.journal.s.computer.run = sul computer del tuo diario, esegui:

journal.window.home.visible.text.only.this.week.changes.the.memory = cambia solo questa settimana. il ricordo resta nel tuo diario.

journal.window.home.visible.text.opens.where.it.came.from.so = si apre da dove viene, così puoi controllarlo.

journal.window.home.visible.text.previous = precedente

journal.window.home.visible.text.say.a.sentence.out.loud = di' una frase ad alta voce

journal.window.home.visible.text.some.source.material.was.unavailable = parte del materiale di origine non era disponibile:

journal.window.home.visible.text.speech.from = parlato da

journal.window.home.visible.text.the.local.model.didn.t.finish = il modello in locale non ha finito di scaricarsi.

journal.window.home.visible.text.the.segment.from = il segmento da

journal.window.home.visible.text.the.speech.model.didn.t.finish = il modello del parlato non ha finito di scaricarsi.

journal.window.home.visible.text.then.type = poi digita

journal.window.home.visible.text.this.isn.t.right = non è giusto

journal.window.home.visible.text.try.again = riprova

journal.window.home.visible.text.waiting.for.the.next.segment.this = in attesa del prossimo segmento. può volerci qualche minuto.

journal.window.home.visible.text.was.written.out = è stato redatto.

journal.window.home.visible.text.what.this.summary.covers.wasn.t = ciò che questo riepilogo copre non è stato salvato con esso.

journal.window.home.visible.text.yesterday.is.being.written.up = ieri si sta redigendo…

journal.window.home.visible.text.your.journal.keeps.everything.until.you = il tuo diario tiene tutto finché non lo fai.

journal.window.home.vitals.last.check.timestamp = ultimo controllo: { $checked_at }.

journal.window.home.vitals.last.known = ultimo noto:{ $slot1 }{ $issue }

journal.window.home.vitals.last.reached.timestamp = ultima connessione al tuo diario { $last_reached_at }

journal.window.home.week.card.empty = non compare niente di questa settimana.

journal.window.home.week.card.first.choose = la tua prima settimana arriva quando scegli dove avviene il ragionamento.

journal.window.home.week.card.first.due.friday = la tua prima settimana arriva venerdì.

journal.window.home.week.card.first.due.monday = la tua prima settimana arriva lunedì.

journal.window.home.week.card.first.due.saturday = la tua prima settimana arriva sabato.

journal.window.home.week.card.first.due.sunday = la tua prima settimana arriva domenica.

journal.window.home.week.card.first.due.thursday = la tua prima settimana arriva giovedì.

journal.window.home.week.card.first.due.tuesday = la tua prima settimana arriva martedì.

journal.window.home.week.card.first.due.wednesday = la tua prima settimana arriva mercoledì.

journal.window.home.week.card.next.choose = la tua prossima settimana arriva quando scegli dove avviene il ragionamento.

journal.window.home.week.card.next.due.friday = la tua prossima settimana arriva venerdì.

journal.window.home.week.card.next.due.monday = la tua prossima settimana arriva lunedì.

journal.window.home.week.card.next.due.saturday = la tua prossima settimana arriva sabato.

journal.window.home.week.card.next.due.sunday = la tua prossima settimana arriva domenica.

journal.window.home.week.card.next.due.thursday = la tua prossima settimana arriva giovedì.

journal.window.home.week.card.next.due.tuesday = la tua prossima settimana arriva martedì.

journal.window.home.week.card.next.due.wednesday = la tua prossima settimana arriva mercoledì.

journal.window.home.week.card.unchecked = non è stato possibile controllare la tua settimana.

journal.window.home.week.cell.accessible.left.out = { $weekday } { $day }, nel tuo diario, nessun ricordo in questa pagina

journal.window.home.week.cell.accessible.memory = { $weekday } { $day }, un ricordo di questo giorno

journal.window.home.week.cell.accessible.not.ready = { $weekday } { $day }, non era pronto in tempo

journal.window.home.week.cell.accessible.nothing.shared = { $weekday } { $day }, niente nel tuo diario

journal.window.home.week.cell.accessible.unknown = { $weekday } { $day },{ $legacy_space }

journal.window.home.week.cell.accessible.unreadable = { $weekday } { $day }, non è stato possibile leggerlo

journal.window.home.week.cell.left.out = nel tuo diario, nessun ricordo in questa pagina

journal.window.home.week.cell.memory = un ricordo da questo giorno

journal.window.home.week.cell.not.ready = non era pronto in tempo

journal.window.home.week.cell.nothing.shared = niente nel tuo diario

journal.window.home.week.cell.unreadable = non è stato possibile leggerlo

journal.window.home.week.day.long.friday = venerdì

journal.window.home.week.day.long.monday = lunedì

journal.window.home.week.day.long.saturday = sabato

journal.window.home.week.day.long.sunday = domenica

journal.window.home.week.day.long.thursday = giovedì

journal.window.home.week.day.long.tuesday = martedì

journal.window.home.week.day.long.wednesday = mercoledì

journal.window.home.week.day.short.fri = ven

journal.window.home.week.day.short.mon = lun

journal.window.home.week.day.short.sat = sab

journal.window.home.week.day.short.sun = dom

journal.window.home.week.day.short.thu = gio

journal.window.home.week.day.short.tue = mar

journal.window.home.week.day.short.wed = mer

journal.window.home.week.day.short.with.number = { $weekday }{ $slot1 }{ $day }

journal.window.home.week.days.conjunction = { $leading }{ $slot1 }e{ $slot2 }{ $last }

journal.window.home.week.days.from = da{ $slot1 }{ $days }.

journal.window.home.week.days.unreadable = qualcosa da{ $slot1 }{ $days },{ $slot2 }non è stato possibile leggerlo.

journal.window.home.week.days.unready = { $quantity ->
    [one] { $days } non era pronto in tempo.
    [many] { $days } non erano pronti in tempo.
   *[other] { $days } non erano pronti in tempo.
}

journal.window.home.week.end = questa è la settimana.

journal.window.home.week.intro.empty = niente di questa settimana è in questa pagina.

journal.window.home.week.intro.memories = { $quantity ->
    [one] la tua settimana, un ricordo da un giorno.
    [many] la tua settimana, un ricordo da ciascuno di { $count } di giorni.
   *[other] la tua settimana, un ricordo da ciascuno di { $count } giorni.
}

journal.window.home.week.leave.out.apply.failed = non è stato possibile lasciarlo fuori. non è cambiato niente.

journal.window.home.week.leave.out.undo.failed = non è stato possibile riportarlo. non è cambiato niente.

journal.window.home.week.left.out.unreadable = non è stato possibile controllare i ricordi lasciati fuori, quindi si mostra tutto.

journal.window.home.week.load.failed = non è stato possibile caricare questa settimana. ricarica per riprovare.

journal.window.home.week.menu.accessible.name = altro per{ $slot1 }{ $day }

journal.window.home.week.month.april = aprile

journal.window.home.week.month.august = agosto

journal.window.home.week.month.december = dicembre

journal.window.home.week.month.february = febbraio

journal.window.home.week.month.january = gennaio

journal.window.home.week.month.july = luglio

journal.window.home.week.month.june = giugno

journal.window.home.week.month.march = marzo

journal.window.home.week.month.may = maggio

journal.window.home.week.month.november = novembre

journal.window.home.week.month.october = ottobre

journal.window.home.week.month.september = settembre

journal.window.home.week.navigation.next = { $week } →

journal.window.home.week.navigation.previous = ← { $week }

journal.window.home.week.peek.briefing = dal riepilogo del giorno per{ $slot1 }{ $day }

journal.window.home.week.peek.moment = apri quel momento →

journal.window.home.week.peek.thinking = aprilo nel ragionamento →

journal.window.home.week.said.heading = detto da te

journal.window.home.week.said.leave.out.scope = cambia solo questa settimana. ciò che hai detto resta nel tuo diario.

journal.window.home.week.said.open = apri dove l'hai detto →

journal.window.home.week.said.unreadable = non è stato possibile leggere l'elenco "detto da te".

journal.window.home.week.source.link = fonte

journal.window.home.week.title.current = settimana del{ $slot1 }{ $day }{ $slot2 }{ $month }

journal.window.home.week.title.with.year = settimana del{ $slot1 }{ $day }{ $slot2 }{ $month }{ $slot3 }{ $year }

journal.window.home.week.unreadable.current = la settimana del{ $slot1 }{ $day }{ $slot2 }{ $month },{ $slot3 }non è stato possibile leggerla.

journal.window.home.week.unreadable.with.year = la settimana del{ $slot1 }{ $day }{ $slot2 }{ $month }{ $slot3 }{ $year }, non è stato possibile leggerla.

journal.window.home.welcome.first.week.framing = la maggior parte di ciò che il tuo diario conserva diventa utile dopo circa una settimana, quando il tuo diario ha abbastanza dei tuoi giorni per mostrare schemi. per ora, ecco cosa sta già succedendo:

journal.window.home.window.explanation.analysis.will.be.available.after.the = l'analisi sarà disponibile dopo il prossimo ciclo di elaborazione.

journal.window.home.window.explanation.backup.status.is.unavailable.right.now = lo stato del backup non è disponibile adesso.

journal.window.home.window.explanation.choose.where.the.thinking.happens = scegli dove avviene il ragionamento

journal.window.home.window.explanation.couldn.t.bring.this.back.nothing = non è stato possibile riportarlo. non è cambiato niente.

journal.window.home.window.explanation.couldn.t.leave.this.out.nothing = non è stato possibile lasciarlo fuori. non è cambiato niente.

journal.window.home.window.explanation.couldn.t.refresh.narrative.showing.last = non è stato possibile aggiornare il racconto. si mostra l'ultimo stato noto.

journal.window.home.window.explanation.couldn.t.refresh.service.status.showing = non è stato possibile aggiornare lo stato del servizio. si mostra l'ultimo stato noto.

journal.window.home.window.explanation.finish.setting.up.confidential.processing = finisci di configurare l'elaborazione confidenziale

journal.window.home.window.explanation.finish.setting.up.local.thinking = finisci di configurare il ragionamento in locale

journal.window.home.window.explanation.finish.setting.up.your.own.model = finisci di configurare il tuo modello

journal.window.home.window.explanation.no.narrative.yet.one.will.appear = ancora nessun racconto. ne apparirà uno dopo che un po' di audio arriva nel tuo diario.

journal.window.home.window.explanation.queued.work.status.is.unavailable.right = lo stato del lavoro in coda non è disponibile in questo momento.

journal.window.home.window.explanation.reload.to.try.again = ricarica per riprovare.

journal.window.home.window.explanation.service.status.is.unavailable.right.now = lo stato del servizio non è disponibile in questo momento.

journal.window.home.window.explanation.this.is.where.your.day.comes = qui il tuo giorno si compone: cosa è successo, il resoconto, e le persone e le cose che ha coinvolto. le sezioni compaiono qui man mano che ciò che condividi con l'app di solstone arriva nel tuo diario.

journal.window.home.window.explanation.time.in.each.facet = tempo in ogni facetta

journal.window.home.window.explanation.yesterday.shows.here.once.confidential.processing = ieri compare qui quando l'elaborazione confidenziale è configurata.

journal.window.home.window.explanation.yesterday.shows.here.once.local.thinking = ieri compare qui quando il ragionamento in locale è configurato.

journal.window.home.window.explanation.yesterday.shows.here.once.you.choose = ieri compare qui quando scegli dove avviene il ragionamento.

journal.window.home.window.explanation.yesterday.shows.here.once.your.own = ieri compare qui quando il tuo modello è configurato.

journal.window.home.window.explanation.yesterday.shows.here.the.morning.after = ieri compare qui la mattina dopo che qualcosa arriva nel tuo diario.

journal.window.home.window.explanation.your.first.day.shows.here.by = il tuo primo giorno compare qui entro le 10 di mattina, dopo la prima notte del tuo diario.

journal.window.home.window.explanation.your.journal.s.computer.can.t = il computer del tuo diario non può eseguire il ragionamento in locale

journal.window.loading.activities = caricamento delle attività…

journal.window.loading.agents = caricamento degli agenti…

journal.window.loading.backup = caricamento del backup…

journal.window.loading.day = caricamento del giorno…

journal.window.loading.entities = caricamento delle entità…

journal.window.loading.facets = caricamento delle facette…

journal.window.loading.health = caricamento dello stato…

journal.window.loading.home = caricamento del tuo giorno…

journal.window.loading.import = caricamento delle importazioni…

journal.window.loading.map = caricamento della mappa…

journal.window.loading.more = caricamento di altro…

journal.window.loading.network = caricamento dei dispositivi…

journal.window.loading.profile = caricamento del profilo…

journal.window.loading.records = caricamento dei registri…

journal.window.loading.search = caricamento della ricerca…

journal.window.loading.settings = caricamento delle impostazioni…

journal.window.loading.sources = caricamento delle fonti…

journal.window.loading.speakers = caricamento dei parlanti in corso…

journal.window.loading.stats = caricamento delle statistiche…

journal.window.loading.support = caricamento del supporto…

journal.window.loading.thinking = caricamento delle impostazioni di ragionamento…

journal.window.loading.transcripts = caricamento delle trascrizioni…

journal.window.loading.year = caricamento dell'anno…

journal.window.more.agents.count = { $quantity ->
    [one] { $count } connesso
    [many] { $count } connessi
   *[other] { $count } connessi
}

journal.window.more.coverage.since = da{ $slot1 }{ $date }

journal.window.more.entities.count = { $quantity ->
    [one] { $count } conservata
    [many] { $count } conservate
   *[other] { $count } conservate
}

journal.window.more.field.label.filter.views = filtra le viste

journal.window.more.field.placeholder.type.to.filter.views = digita per filtrare le viste

journal.window.more.group.runs = come si esegue

journal.window.more.group.shape = ciò a cui dai forma

journal.window.more.imports.count = { $quantity ->
    [one] 1 importazione
    [many] { $count } di importazioni
   *[other] { $count } importazioni
}

journal.window.more.page.heading.more = altro

journal.window.more.pin.action.pin = { $view ->
    [agents] fissa gli agenti alla barra laterale
    [activities] fissa le attività alla barra laterale
    [backup] fissa il backup alla barra laterale
    [body] fissa il corpo alla barra laterale
    [curation] fissa i suggerimenti alla barra laterale
    [entities] fissa le entità alla barra laterale
    [health] fissa lo stato alla barra laterale
    [home] fissa l'inizio alla barra laterale
    [import] fissa l'importazione alla barra laterale
    [map] fissa la mappa alla barra laterale
    [more] fissa altro alla barra laterale
    [network] fissa la rete alla barra laterale
    [search] fissa la ricerca alla barra laterale
    [settings] fissa le impostazioni alla barra laterale
    [sources] fissa le fonti alla barra laterale
    [speakers] fissa i parlanti alla barra laterale
    [stats] fissa le statistiche alla barra laterale
    [support] fissa il supporto alla barra laterale
    [thinking] fissa il ragionamento alla barra laterale
    [transcripts] fissa le trascrizioni alla barra laterale
    [year] fissa l'anno alla barra laterale
   *[other] fissa { $view_name } alla barra laterale
}

journal.window.more.pin.action.unpin = { $view ->
    [agents] togli gli agenti dalla barra laterale
    [activities] togli le attività dalla barra laterale
    [backup] togli il backup dalla barra laterale
    [body] togli il corpo dalla barra laterale
    [curation] togli i suggerimenti dalla barra laterale
    [entities] togli le entità dalla barra laterale
    [health] togli lo stato dalla barra laterale
    [home] togli l'inizio dalla barra laterale
    [import] togli l'importazione dalla barra laterale
    [map] togli la mappa dalla barra laterale
    [more] togli altro dalla barra laterale
    [network] togli la rete dalla barra laterale
    [search] togli la ricerca dalla barra laterale
    [settings] togli le impostazioni dalla barra laterale
    [sources] togli le fonti dalla barra laterale
    [speakers] togli i parlanti dalla barra laterale
    [stats] togli le statistiche dalla barra laterale
    [support] togli il supporto dalla barra laterale
    [thinking] togli il ragionamento dalla barra laterale
    [transcripts] togli le trascrizioni dalla barra laterale
    [year] togli l'anno dalla barra laterale
   *[other] togli { $view_name } dalla barra laterale
}

journal.window.more.review.count = { $quantity ->
    [one] { $count } da rivedere
    [many] { $count } da rivedere
   *[other] { $count } da rivedere
}

journal.window.more.suggestions.pending = { $count }{ $slot1 }da rivedere

journal.window.more.text.no.devices.yet = ancora nessun dispositivo

journal.window.more.text.none.yet = ancora niente

journal.window.more.text.not.set.up.yet = non ancora configurato

journal.window.more.text.nothing.to.review = niente da rivedere

journal.window.more.text.nothing.yet = ancora niente

journal.window.more.tooltip.always.in.the.side.rail = sempre nella barra laterale

journal.window.more.visible.text.couldn.t.save.that.try.again = non è stato possibile salvare. riprova.

journal.window.more.visible.text.every.view.your.journal.has.what = ogni vista del tuo diario: cosa mostra ciascuna, e da dove arrivano i suoi contenuti.

journal.window.more.visible.text.no.views.match.that = nessuna vista corrisponde a quello.

journal.window.more.window.explanation.fix.and.tidy.what.it.keeps = sistema e riordina ciò che conserva

journal.window.more.window.explanation.set.it.up.and.keep.it = configuralo e mantienilo in esecuzione

journal.window.more.window.explanation.what.your.journal.keeps = ciò che il tuo diario conserva

journal.window.more.window.explanation.your.days.as.your.journal.holds = i tuoi giorni, come li tiene il tuo diario

journal.window.search.agent.action = azione

journal.window.search.agent.activity = attività

journal.window.search.agent.audio = audio

journal.window.search.agent.audio.transcript = trascrizione dell'audio

journal.window.search.agent.body = corpo

journal.window.search.agent.brief = sintesi

journal.window.search.agent.browser = browser

journal.window.search.agent.calendar = calendario

journal.window.search.agent.conversation.transcript = trascrizione della conversazione

journal.window.search.agent.describe = descrizione

journal.window.search.agent.desktop.activity = attività sul desktop

journal.window.search.agent.document.transcript = trascrizione del documento

journal.window.search.agent.documents = documenti

journal.window.search.agent.entity.attached = entità:associata

journal.window.search.agent.entity.detected = entità:rilevata

journal.window.search.agent.event = evento

journal.window.search.agent.event.transcript = trascrizione dell'evento

journal.window.search.agent.flow = flusso

journal.window.search.agent.granola = granola

journal.window.search.agent.image.transcript = trascrizione dell'immagine

journal.window.search.agent.import = importazione

journal.window.search.agent.imported = importato

journal.window.search.agent.location = posizione

journal.window.search.agent.memory = ricordo

journal.window.search.agent.morning.briefing = riepilogo del mattino

journal.window.search.agent.note.transcript = trascrizione della nota

journal.window.search.agent.objects = oggetti

journal.window.search.agent.observation = osservazione

journal.window.search.agent.possible.names = nomi possibili

journal.window.search.agent.protected.video = video protetto

journal.window.search.agent.pulse = impulso

journal.window.search.agent.reflection = riflessione

journal.window.search.agent.screen = schermo

journal.window.search.agent.screen.text = testo dello schermo

journal.window.search.agent.screenshot.transcript = trascrizione dello screenshot

journal.window.search.agent.sense = senso

journal.window.search.agent.session.review = revisione della sessione

journal.window.search.agent.story = racconto

journal.window.search.agent.topics = argomenti

journal.window.search.count.results.across.days = { $match_quantity ->
    [one] { $day_quantity ->
        [one] { $match_count } corrispondenza in { $day_count } giorno.
        [many] { $match_count } corrispondenza in { $day_count } di giorni.
       *[other] { $match_count } corrispondenza in { $day_count } giorni.
    }
    [many] { $day_quantity ->
        [one] { $match_count } di corrispondenze in { $day_count } giorno.
        [many] { $match_count } di corrispondenze in { $day_count } di giorni.
       *[other] { $match_count } di corrispondenze in { $day_count } giorni.
    }
   *[other] { $day_quantity ->
        [one] { $match_count } corrispondenze in { $day_count } giorno.
        [many] { $match_count } corrispondenze in { $day_count } di giorni.
       *[other] { $match_count } corrispondenze in { $day_count } giorni.
    }
}

journal.window.search.count.results.shown.for.day = { $match_quantity ->
    [one] { $shown_quantity ->
        [one] si mostrano le prime { $shown_count } di { $match_count } corrispondenza per questo giorno.
        [many] si mostrano le prime { $shown_count } di { $match_count } corrispondenza per questo giorno.
       *[other] si mostrano le prime { $shown_count } di { $match_count } corrispondenza per questo giorno.
    }
    [many] { $shown_quantity ->
        [one] si mostrano le prime { $shown_count } di { $match_count } di corrispondenze per questo giorno.
        [many] si mostrano le prime { $shown_count } di { $match_count } di corrispondenze per questo giorno.
       *[other] si mostrano le prime { $shown_count } di { $match_count } di corrispondenze per questo giorno.
    }
   *[other] { $shown_quantity ->
        [one] si mostrano le prime { $shown_count } di { $match_count } corrispondenze per questo giorno.
        [many] si mostrano le prime { $shown_count } di { $match_count } corrispondenze per questo giorno.
       *[other] si mostrano le prime { $shown_count } di { $match_count } corrispondenze per questo giorno.
    }
}

journal.window.search.date.from = da

journal.window.search.date.invalid = scegli una data di fine uguale o successiva alla data di inizio.

journal.window.search.date.range = intervallo di date

journal.window.search.date.through = fino a

journal.window.search.entry.as.indexed = come all'ultima indicizzazione

journal.window.search.entry.close = chiudi l'elemento

journal.window.search.entry.loading = caricamento dell'elemento…

journal.window.search.entry.open = leggi l'elemento corrispondente

journal.window.search.entry.reindexed = questo file è stato indicizzato di nuovo dopo la tua ricerca, quindi potrebbe non corrispondere al risultato

journal.window.search.entry.stored.record = il registro salvato

journal.window.search.heading = cerca nel tuo diario

journal.window.search.hit.names = nomi

journal.window.search.intro = trova parole e frasi nel tuo diario, poi leggi l'elemento corrispondente.

journal.window.search.link.agents = apri negli agenti

journal.window.search.link.day = apri quel giorno →

journal.window.search.link.entity = apri l'entità

journal.window.search.link.map = apri sulla mappa

journal.window.search.link.transcript = apri la trascrizione

journal.window.search.link.week = apri quella settimana →

journal.window.search.message.assistant = Assistente

journal.window.search.message.everyone = tutti

journal.window.search.message.meta = { $sender } a { $recipients }

journal.window.search.message.unknown = Sconosciuto

journal.window.search.message.you = tu

journal.window.search.paging.accessible.name = pagine dei risultati di ricerca

journal.window.search.paging.day = altro da questo giorno

journal.window.search.paging.first = torna ai primi risultati

journal.window.search.paging.newer = giorni corrispondenti più recenti

journal.window.search.paging.older = giorni corrispondenti più vecchi

journal.window.search.query.label = parole o frase

journal.window.search.query.placeholder = cosa vorresti trovare?

journal.window.search.query.submit = cerca

journal.window.search.response.agent.name = l'agente deve essere solo il nome del talento

journal.window.search.response.date.format = { $field }{ $slot1 }deve essere { $slot2 }

journal.window.search.response.date.real = { $field }{ $slot1 }deve essere un giorno reale

journal.window.search.response.date.reverse = { $slot1 } deve essere <= { $slot2 }

journal.window.search.response.day.format = { $slot1 } deve essere { $slot2 }

journal.window.search.response.entry.absent = questo elemento non è più indicizzato. cerca di nuovo per aggiornare i risultati.

journal.window.search.response.entry.invalid = riferimento non valido all'elemento

journal.window.search.response.entry.large = questo elemento è troppo lungo da mostrare qui.

journal.window.search.response.file.not.found = quel file non è disponibile.

journal.window.search.response.file.read.failed = non è stato possibile leggere quel file.

journal.window.search.response.filter.agent = non esiste un agente di nome{ $slot1 }{ $name }. agenti:{ $slot2 }{ $available }

journal.window.search.response.filter.facet = non esiste una facetta di nome{ $slot1 }{ $name }. facette:{ $slot2 }{ $available }

journal.window.search.response.index.read.detail = non è stato possibile leggere l'indice di ricerca. riprova.

journal.window.search.response.invalid.day = non è stato possibile usare quel giorno.

journal.window.search.response.invalid.path = non è stato possibile usare quel percorso.

journal.window.search.response.invalid.value = non è stato possibile usare uno di quei valori.

journal.window.search.response.read.agent.absent = non è stato trovato nessun output di quel tipo per quel giorno

journal.window.search.response.read.bound = { $slot1 } deve essere { $slot2 } per le letture { $slot3 }

journal.window.search.response.read.entity = i risultati dell'entità non hanno un file da leggere

journal.window.search.response.read.entry.fields = un elemento indicizzato richiede { $slot1 }, { $slot2 } e { $slot3 } dallo stesso risultato di ricerca

journal.window.search.response.read.failed = non è stato possibile leggere il file del diario

journal.window.search.response.read.file.absent = file del diario non trovato

journal.window.search.response.read.mixed.entry = un elemento indicizzato non può essere combinato con { $slot1 }, { $slot2 } o { $slot3 }

journal.window.search.response.read.mixed.file = { $slot1 } non può essere combinato con { $slot2 }, { $slot3 } o { $slot4 }

journal.window.search.response.read.required = { $slot1 } o { $slot2 } è richiesto

journal.window.search.response.read.suffix = rimuovi il suffisso { $slot1 } del risultato di ricerca prima di leggere un percorso

journal.window.search.response.search.failed = non è stato possibile cercare nel tuo diario in questo momento.

journal.window.search.response.segment.invalid = chiave del segmento non valida

journal.window.search.status.details = dettagli

journal.window.search.status.entry.failed = non è stato possibile aprire questo elemento.{ $slot1 }{ $detail }

journal.window.search.status.entry.failure.details = non è stato possibile aprire questo elemento. { $detail } { $diagnostic }

journal.window.search.status.entry.failure.retry = non è stato possibile aprire questo elemento. riprova.

journal.window.search.status.entry.failure.retry.diagnostic = non è stato possibile aprire questo elemento. riprova. { $diagnostic }

journal.window.search.status.failure.index.absent = non è stato possibile cercare nel tuo diario: il tuo diario non ha ancora un indice di ricerca.

journal.window.search.status.failure.index.empty = non è stato possibile cercare nel tuo diario: non c'è ancora niente nell'indice.

journal.window.search.status.failure.index.locked = non è stato possibile cercare nel tuo diario: l'indice di ricerca è occupato in questo momento.

journal.window.search.status.failure.index.unreadable = non è stato possibile cercare nel tuo diario: non è stato possibile leggere l'indice di ricerca.

journal.window.search.status.failure.retry = non è stato possibile cercare nel tuo diario. riprova.

journal.window.search.status.index.rebuilding = la ricerca si sta ricostruendo, quindi i giorni più vecchi possono mancare per ora.

journal.window.search.status.initial = cerca in tutto il tuo diario. cita una "frase esatta", o restringi all'intervallo di giorni sopra per risultati più rapidi.

journal.window.search.status.loading = cercando nel tuo diario…

journal.window.search.status.narrow = aggiungi parole per restringere queste corrispondenze.

journal.window.search.status.none = nessuna corrispondenza. prova con meno parole o un intervallo di date più ampio.

journal.window.search.status.progress = ancora in ricerca…{ $slot1 }{ $seconds }{ $slot2 }s.

journal.window.search.status.progress.all.days = ancora in ricerca…{ $slot1 }{ $seconds }{ $slot2 }s. un solo giorno di solito è molto più veloce.

journal.window.search.status.relaxed = niente ha corrisposto alla tua ricerca esatta, quindi queste sono corrispondenze più ampie.

journal.window.search.status.retry = riprova

journal.window.search.status.search.failed.other = non è stato possibile cercare nel tuo diario.{ $slot1 }{ $detail }

journal.window.shared.api.authentication.required = Autenticazione richiesta

journal.window.shared.api.malformed.response = Risposta del server malformata

journal.window.shared.api.request.failed = Richiesta non riuscita

journal.window.shared.api.request.failed.http = Richiesta non riuscita (HTTP{ $slot1 }{ $http_status })

journal.window.shared.api.window.explanation.couldn.t.finish.that.request = non è stato possibile finire quella richiesta.

journal.window.shared.app.disclosure.label.show.details = mostra i dettagli

journal.window.shared.app.label.try.now = Prova ora

journal.window.shared.app.notice.background.task = { $app_name }{ $slot1 }compito in secondo piano

journal.window.shared.app.notification.segments = { $count }{ $slot1 }segmenti

journal.window.shared.app.notification.title = Notifica

journal.window.shared.app.request.failed = Richiesta non riuscita

journal.window.shared.app.visible.text.click.to.copy = (clicca per copiare)

journal.window.shared.app.visible.text.copied = (copiato)

journal.window.shared.app.visible.text.reason.code = codice del motivo:

journal.window.shared.app.visible.text.reference = riferimento:

journal.window.shared.app.visible.text.server.reason = Motivo del server:

journal.window.shared.app.visible.text.still.working = ancora in corso…

journal.window.shared.app.visible.text.time = ora:

journal.window.shared.app.window.explanation.also.remove.it.from.your.backups = rimuovilo anche dai tuoi backup

journal.window.shared.app.window.explanation.also.remove.them.from.your.backups = rimuovili anche dai tuoi backup

journal.window.shared.app.window.explanation.backup.is.paused.so.this.happens = il backup è in pausa, quindi questo avviene quando lo riprendi.

journal.window.shared.app.window.explanation.couldn.t.load.this.section = non è stato possibile caricare questa sezione

journal.window.shared.app.window.explanation.couldn.t.reach.your.journal.some = non è stato possibile connettersi al tuo diario. alcuni aggiornamenti in secondo piano sono in pausa.

journal.window.shared.app.window.explanation.reload.to.try.again = ricarica per riprovare.

journal.window.shared.app.window.explanation.removing.it.from.your.backups.next = viene rimosso dai tuoi backup al passo successivo.

journal.window.shared.app.window.explanation.removing.it.from.your.backups.once = viene rimosso dai tuoi backup quando riprendi il backup.

journal.window.shared.app.window.explanation.removing.them.from.your.backups.next = vengono rimossi dai tuoi backup al passo successivo.

journal.window.shared.app.window.explanation.removing.them.from.your.backups.once = vengono rimossi dai tuoi backup quando riprendi il backup.

journal.window.shared.app.window.explanation.then.restoring.your.journal.from.a = poi il ripristino del tuo diario da un backup non può riportarlo. i tuoi backup restano, con tutto il resto che contengono.

journal.window.shared.app.window.explanation.then.restoring.your.journal.from.a.variant2 = poi il ripristino del tuo diario da un backup non può riportarli. i tuoi backup restano, con tutto il resto che contengono.

journal.window.shared.app.window.explanation.your.journal.couldn.t.check.your = il tuo diario non ha potuto controllare i tuoi backup proprio ora, quindi questo li lascia come sono.

journal.window.shared.console.link.message.count = { $quantity ->
    [one] messaggio di sistema ({ $count })
    [many] messaggi di sistema ({ $count })
   *[other] messaggi di sistema ({ $count })
}

journal.window.shared.console.link.unread.count = { $quantity ->
    [one] messaggi di sistema, { $count } non letto
    [many] messaggi di sistema, { $count } non letti
   *[other] messaggi di sistema, { $count } non letti
}

journal.window.shared.console.snapshot.message.count = { $quantity ->
    [one] istantanea di { $count } messaggio di sistema
    [many] istantanea di { $count } di messaggi di sistema
   *[other] istantanea di { $count } messaggi di sistema
}

journal.window.shared.date.format.duration.hour.and.minute.count = { $quantity ->
    [one] { $hours } h { $minutes_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
    [many] { $hours } h { $minutes_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
   *[other] { $hours } h { $minutes_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
}

journal.window.shared.date.format.duration.hour.count = { $quantity ->
    [one] { $hours } h
    [many] { $hours } h
   *[other] { $hours } h
}

journal.window.shared.date.format.duration.minute.and.second.count = { $quantity ->
    [one] { $minutes } min { $seconds_quantity ->
        [one] { $seconds } s
        [many] { $seconds } s
       *[other] { $seconds } s
    }
    [many] { $minutes } min { $seconds_quantity ->
        [one] { $seconds } s
        [many] { $seconds } s
       *[other] { $seconds } s
    }
   *[other] { $minutes } min { $seconds_quantity ->
        [one] { $seconds } s
        [many] { $seconds } s
       *[other] { $seconds } s
    }
}

journal.window.shared.date.format.duration.minute.count = { $quantity ->
    [one] { $minutes } min
    [many] { $minutes } min
   *[other] { $minutes } min
}

journal.window.shared.date.format.duration.second.count = { $quantity ->
    [one] { $seconds } s
    [many] { $seconds } s
   *[other] { $seconds } s
}

journal.window.shared.date.format.duration.unavailable = durata non disponibile

journal.window.shared.date.format.import.audio = audio

journal.window.shared.date.format.import.document = documenti

journal.window.shared.date.format.import.ics = calendario

journal.window.shared.date.format.import.image = immagini

journal.window.shared.date.format.import.journal.archive = un altro diario

journal.window.shared.date.format.import.obsidian = note

journal.window.shared.date.format.imported.source.label = { $source_name }{ $slot1 }· importato

journal.window.shared.date.format.lane.byo = il tuo modello

journal.window.shared.date.format.lane.default = elaborazione

journal.window.shared.date.format.lane.local = ragionamento in locale

journal.window.shared.date.format.lane.spp = elaborazione confidenziale

journal.window.shared.date.format.months.since.apr = apr

journal.window.shared.date.format.months.since.aug = ago

journal.window.shared.date.format.months.since.dec = dic

journal.window.shared.date.format.months.since.feb = feb

journal.window.shared.date.format.months.since.jan = gen

journal.window.shared.date.format.months.since.jul = lug

journal.window.shared.date.format.months.since.jun = giu

journal.window.shared.date.format.months.since.mar = mar

journal.window.shared.date.format.months.since.may = mag

journal.window.shared.date.format.months.since.nov = nov

journal.window.shared.date.format.months.since.oct = ott

journal.window.shared.date.format.months.since.sep = set

journal.window.shared.date.format.source.browser = il tuo browser

journal.window.shared.date.format.source.phone = il tuo telefono

journal.window.shared.date.format.source.terminal = il tuo terminale

journal.window.shared.date.format.source.unknown = fonte sconosciuta

journal.window.shared.date.format.source.watch = il tuo orologio

journal.window.shared.date.format.time.unavailable = ora non disponibile

journal.window.shared.date.nav.accessible.name.change.date.level = cambia il livello della data

journal.window.shared.date.nav.accessible.name.go.to.today = vai a oggi

journal.window.shared.date.nav.accessible.name.next = avanti

journal.window.shared.date.nav.accessible.name.next.day = giorno successivo

journal.window.shared.date.nav.accessible.name.previous = precedente

journal.window.shared.date.nav.accessible.name.previous.day = giorno precedente

journal.window.shared.date.nav.cell.date.and.count = { $day_label }, { $quantity_label }

journal.window.shared.date.nav.days.counting = conteggio dei giorni ancora in corso…

journal.window.shared.date.nav.days.loading = caricamento dei giorni…

journal.window.shared.date.nav.index.failed.day.count = { $quantity ->
    [one] non è stato possibile contare 1 giorno. si sta ancora provando.
    [many] non è stato possibile contare { $count } di giorni. si sta ancora provando.
   *[other] non è stato possibile contare { $count } giorni. si sta ancora provando.
}

journal.window.shared.date.nav.months.label.april = Aprile

journal.window.shared.date.nav.months.label.august = Agosto

journal.window.shared.date.nav.months.label.december = Dicembre

journal.window.shared.date.nav.months.label.february = Febbraio

journal.window.shared.date.nav.months.label.january = Gennaio

journal.window.shared.date.nav.months.label.july = Luglio

journal.window.shared.date.nav.months.label.june = Giugno

journal.window.shared.date.nav.months.label.march = Marzo

journal.window.shared.date.nav.months.label.may = Maggio

journal.window.shared.date.nav.months.label.november = Novembre

journal.window.shared.date.nav.months.label.october = Ottobre

journal.window.shared.date.nav.months.label.september = Settembre

journal.window.shared.date.nav.months.loading = caricamento dei mesi…

journal.window.shared.date.nav.visible.text.pick.a.day = scegli un giorno

journal.window.shared.date.nav.visible.text.today = oggi

journal.window.shared.date.nav.visible.text.try.again = riprova

journal.window.shared.date.nav.week.current = Questa settimana

journal.window.shared.date.nav.week.headline.month.day = settimana del{ $slot1 }{ $day }{ $slot2 }{ $month }

journal.window.shared.date.nav.week.option.month.day = settimana del{ $slot1 }{ $day }{ $slot2 }{ $month }

journal.window.shared.date.nav.week.previous = La settimana scorsa

journal.window.shared.date.nav.window.explanation.couldn.t.load.which.days.have = non è stato possibile caricare quali giorni hanno materiale.

journal.window.shared.date.nav.window.explanation.couldn.t.load.which.months.have = non è stato possibile caricare quali mesi hanno materiale.

journal.window.shared.day.grid.calendar.year = { $year }{ $slot1 }calendario

journal.window.shared.day.grid.cell.date.and.count = { $day_label }: { $quantity_label }

journal.window.shared.day.grid.cell.date.and.pending.count = { $day_label }: { $quantity_label }, riepilogo in attesa

journal.window.shared.day.grid.range.anchor = completa l'intervallo da{ $slot1 }{ $start_day }

journal.window.shared.day.grid.range.cleared = intervallo azzerato

journal.window.shared.day.grid.range.start = l'intervallo inizia{ $slot1 }{ $start_day }. scegli l'ultimo giorno.

journal.window.shared.day.grid.visible.text.less = meno

journal.window.shared.day.grid.visible.text.more = altro

journal.window.shared.day.grid.visible.text.none = nessuno

journal.window.shared.day.grid.visible.text.open = apri →

journal.window.shared.day.grid.visible.text.pending.rollup = riepilogo in attesa

journal.window.shared.day.grid.visible.text.yes = sì

journal.window.shared.diagnostic.console.reason.code = codice del motivo

journal.window.shared.diagnostic.console.server.reason = Motivo del server

journal.window.shared.diagnostic.console.visible.text.http = · HTTP

journal.window.shared.error.handler.window.explanation.couldn.t.load.part.of.this = non è stato possibile caricare una parte di questa pagina.

journal.window.shared.error.handler.window.explanation.navigation.aborted.a.read = la navigazione ha interrotto una lettura:

journal.window.shared.error.handler.window.explanation.something.on.this.page.didn.t = qualcosa in questa pagina non ha funzionato.

journal.window.shared.gate.drawer.audio.recordings.count = { $quantity ->
    [one] registrazione con audio: { $count }
    [many] registrazioni con audio: { $count }
   *[other] registrazioni con audio: { $count }
}

journal.window.shared.gate.drawer.manual.tags.count = { $quantity ->
    [one] frase che hai insegnato: { $count }
    [many] frasi che hai insegnato: { $count }
   *[other] frasi che hai insegnato: { $count }
}

journal.window.shared.gate.drawer.median.length = lunghezza mediana

journal.window.shared.gate.drawer.open.label = perché non ancora?

journal.window.shared.gate.drawer.statement.median.required.seconds = lunghezza mediana della frase { $observed }s, servono { $minimum }s

journal.window.shared.gate.drawer.statement.median.seconds = lunghezza mediana della frase { $observed }s

journal.window.shared.gate.drawer.statement.median.unavailable = lunghezza mediana della frase n/a

journal.window.shared.gate.drawer.statement.median.unavailable.required.seconds = lunghezza mediana della frase n/a, servono { $minimum }s

journal.window.shared.gate.drawer.statements.observed.and.required.count = { $quantity ->
    [one] { $observed } frase più lunga finora, { $minimum_quantity ->
        [one] ne serve { $minimum }
        [many] ne servono { $minimum }
       *[other] ne servono { $minimum }
    }
    [many] { $observed } di frasi più lunghe finora, { $minimum_quantity ->
        [one] ne serve { $minimum }
        [many] ne servono { $minimum }
       *[other] ne servono { $minimum }
    }
   *[other] { $observed } frasi più lunghe finora, { $minimum_quantity ->
        [one] ne serve { $minimum }
        [many] ne servono { $minimum }
       *[other] ne servono { $minimum }
    }
}

journal.window.shared.gate.drawer.statements.observed.count = { $quantity ->
    [one] { $observed } frase più lunga finora
    [many] { $observed } di frasi più lunghe finora
   *[other] { $observed } frasi più lunghe finora
}

journal.window.shared.gate.drawer.threshold = servono { $threshold }

journal.window.shared.gate.drawer.window.explanation.pick.longer.statements.when.you.teach = scegli frasi più lunghe quando insegni la tua voce.

journal.window.shared.gate.drawer.window.explanation.pick.statements.where.only.you.are = scegli frasi in cui parli solo tu.

journal.window.shared.gate.drawer.window.explanation.teach.a.few.more.of.your = insegna ancora alcune delle tue frasi più lunghe.

journal.window.shared.gate.drawer.window.explanation.voice.pattern.is.still.too.spread = il pattern vocale è ancora troppo disperso

journal.window.shared.gate.drawer.window.explanation.you.ve.taught.enough.to.save = hai insegnato abbastanza per salvare la tua voce.

journal.window.shared.import.item.dated.label = { $label } · { $date }

journal.window.shared.import.item.kind.conversation = conversazione

journal.window.shared.import.item.kind.document = documento

journal.window.shared.import.item.kind.messages = messaggi

journal.window.shared.import.item.kind.note = nota

journal.window.shared.import.item.kind.photo = foto

journal.window.shared.import.item.kind.screenshot = screenshot

journal.window.shared.import.item.kind.transcript = trascrizione

journal.window.shared.import.item.label = { $kind } · { $source }

journal.window.shared.import.item.source.chatgpt = ChatGPT

journal.window.shared.import.item.source.claude = Claude

journal.window.shared.import.item.source.evernote = Evernote

journal.window.shared.import.item.source.gemini = Gemini

journal.window.shared.import.item.source.imported = importato

journal.window.shared.import.item.source.obsidian = Obsidian

journal.window.shared.import.item.source.pdf = PDF

journal.window.shared.import.item.text.unavailable = nessun testo disponibile

journal.window.shared.mount.workspace.button.label.try.again = riprova

journal.window.shared.mount.workspace.section.heading.couldn.t.load.this.section = non è stato possibile caricare questa sezione

journal.window.shared.mount.workspace.visible.text.reload.to.try.again = ricarica per riprovare.

journal.window.shared.question.visible.text.cancel = annulla

journal.window.shared.question.visible.text.name = nome

journal.window.shared.report.error.journal.version = diario{ $slot1 }{ $version }

journal.window.shared.report.error.status.text.your.versions = le tue versioni

journal.window.shared.report.error.window.explanation.nothing.leaves.this.journal.until.you = non esce niente da questo diario finché non invii il modulo sul sito di supporto.

journal.window.shared.shell.boot.accessible.name.about = informazioni

journal.window.shared.shell.boot.accessible.name.connecting = connessione in corso

journal.window.shared.shell.boot.button.label.try.again = riprova

journal.window.shared.shell.boot.section.heading.couldn.t.load.this.section = non è stato possibile caricare questa sezione

journal.window.shared.shell.boot.tooltip.connecting = connessione in corso

journal.window.shared.shell.boot.visible.text.more = altro

journal.window.shared.shell.boot.visible.text.reload.to.try.again = ricarica per riprovare.

journal.window.shared.shell.boot.window.explanation.no.system.messages.this.session = nessun messaggio di sistema in questa sessione.

journal.window.shared.shell.boot.window.explanation.sending.reports.is.off = l'invio delle segnalazioni è disattivo.

journal.window.shared.status.pane.accessible.label.browser.notifications.enabled = notifiche del browser attive

journal.window.shared.status.pane.accessible.label.enable.browser.notifications = attiva le notifiche del browser

journal.window.shared.status.pane.accessible.label.notifications.blocked.update.in.browser.settings = notifiche bloccate. aggiorna nelle impostazioni del browser

journal.window.shared.status.pane.age.elapsed.day = { $quantity ->
    [one] { $count } giorno fa
    [many] { $count } di giorni fa
   *[other] { $count } giorni fa
}

journal.window.shared.status.pane.age.elapsed.hour = { $quantity ->
    [one] { $count } ora fa
    [many] { $count } di ore fa
   *[other] { $count } ore fa
}

journal.window.shared.status.pane.age.elapsed.minute = { $quantity ->
    [one] { $count } minuto fa
    [many] { $count } di minuti fa
   *[other] { $count } minuti fa
}

journal.window.shared.status.pane.age.elapsed.month = { $quantity ->
    [one] { $count } mese fa
    [many] { $count } di mesi fa
   *[other] { $count } mesi fa
}

journal.window.shared.status.pane.age.elapsed.second = { $quantity ->
    [one] { $count } secondo fa
    [many] { $count } di secondi fa
   *[other] { $count } secondi fa
}

journal.window.shared.status.pane.age.elapsed.week = { $quantity ->
    [one] { $count } settimana fa
    [many] { $count } di settimane fa
   *[other] { $count } settimane fa
}

journal.window.shared.status.pane.automatic.decline = no grazie

journal.window.shared.status.pane.automatic.enable = attiva

journal.window.shared.status.pane.automatic.held = gli aggiornamenti sono trattenuti

journal.window.shared.status.pane.automatic.installs = si installa automaticamente

journal.window.shared.status.pane.automatic.paused = gli aggiornamenti automatici si sono fermati dopo due tentativi che non hanno funzionato.

journal.window.shared.status.pane.automatic.retry = riprova

journal.window.shared.status.pane.automatic.updates = si aggiorna automaticamente

journal.window.shared.status.pane.capture.device.attention = { $device } richiede attenzione

journal.window.shared.status.pane.capture.last.reached.day = ciò che aggiunge non è arrivato nel tuo diario da { $since_day }.

journal.window.shared.status.pane.capture.more.need.attention = { $quantity ->
    [one] e { $count } altro richiede attenzione
    [many] e { $count } di altri richiedono attenzione
   *[other] e { $count } altri richiedono attenzione
}

journal.window.shared.status.pane.capture.rejected.upload.count = { $quantity ->
    [one] 1 caricamento respinto.
    [many] { $count } di caricamenti respinti.
   *[other] { $count } caricamenti respinti.
}

journal.window.shared.status.pane.connected.elapsed.day = { $quantity ->
    [one] connesso da { $count } giorno
    [many] connesso da { $count } di giorni
   *[other] connesso da { $count } giorni
}

journal.window.shared.status.pane.connected.elapsed.hour = { $quantity ->
    [one] connesso da { $count } ora
    [many] connesso da { $count } di ore
   *[other] connesso da { $count } ore
}

journal.window.shared.status.pane.connected.elapsed.minute = { $quantity ->
    [one] connesso da { $count } minuto
    [many] connesso da { $count } di minuti
   *[other] connesso da { $count } minuti
}

journal.window.shared.status.pane.connected.elapsed.month = { $quantity ->
    [one] connesso da { $count } mese
    [many] connesso da { $count } di mesi
   *[other] connesso da { $count } mesi
}

journal.window.shared.status.pane.connected.elapsed.second = { $quantity ->
    [one] connesso da { $count } secondo
    [many] connesso da { $count } di secondi
   *[other] connesso da { $count } secondi
}

journal.window.shared.status.pane.connected.elapsed.week = { $quantity ->
    [one] connesso da { $count } settimana
    [many] connesso da { $count } di settimane
   *[other] connesso da { $count } settimane
}

journal.window.shared.status.pane.connection.just.now = connesso proprio ora

journal.window.shared.status.pane.rejected.elapsed.day = { $quantity ->
    [one] ultimo rifiuto { $count } giorno fa
    [many] ultimo rifiuto { $count } di giorni fa
   *[other] ultimo rifiuto { $count } giorni fa
}

journal.window.shared.status.pane.rejected.elapsed.hour = { $quantity ->
    [one] ultimo rifiuto { $count } ora fa
    [many] ultimo rifiuto { $count } di ore fa
   *[other] ultimo rifiuto { $count } ore fa
}

journal.window.shared.status.pane.rejected.elapsed.minute = { $quantity ->
    [one] ultimo rifiuto { $count } minuto fa
    [many] ultimo rifiuto { $count } di minuti fa
   *[other] ultimo rifiuto { $count } minuti fa
}

journal.window.shared.status.pane.rejected.elapsed.month = { $quantity ->
    [one] ultimo rifiuto { $count } mese fa
    [many] ultimo rifiuto { $count } di mesi fa
   *[other] ultimo rifiuto { $count } mesi fa
}

journal.window.shared.status.pane.rejected.elapsed.second = { $quantity ->
    [one] ultimo rifiuto { $count } secondo fa
    [many] ultimo rifiuto { $count } di secondi fa
   *[other] ultimo rifiuto { $count } secondi fa
}

journal.window.shared.status.pane.rejected.elapsed.week = { $quantity ->
    [one] ultimo rifiuto { $count } settimana fa
    [many] ultimo rifiuto { $count } di settimane fa
   *[other] ultimo rifiuto { $count } settimane fa
}

journal.window.shared.status.pane.security.device.update.version = all'app di solstone su { $device_name } manca una correzione di sicurezza (aggiorna a { $fixed_version }).

journal.window.shared.status.pane.security.journal.update.version = al tuo diario manca una correzione di sicurezza (aggiorna a { $fixed_version }).

journal.window.shared.status.pane.security.restart = riavvia il tuo diario

journal.window.shared.status.pane.security.unnamed.device.update.version = all'app di solstone su uno dei tuoi dispositivi manca una correzione di sicurezza (aggiorna a { $fixed_version }).

journal.window.shared.status.pane.status.text.a.device.isn.t.reaching.your = un dispositivo non si connette al tuo diario.

journal.window.shared.status.pane.status.text.a.device.needs.attention = un dispositivo richiede attenzione

journal.window.shared.status.pane.status.text.update.or.restart.it.on.that = aggiornala o riavviala su quel dispositivo, poi un caricamento valido risolve questo.

journal.window.shared.status.pane.tooltip.browser.notifications.enabled = notifiche del browser attive

journal.window.shared.status.pane.tooltip.enable.browser.notifications = attiva le notifiche del browser

journal.window.shared.status.pane.tooltip.notifications.blocked.update.in.browser.settings = notifiche bloccate. aggiorna nelle impostazioni del browser

journal.window.shared.status.pane.update.just.now = aggiornato proprio ora

journal.window.shared.status.pane.updated.elapsed.day = { $quantity ->
    [one] aggiornato { $count } giorno fa
    [many] aggiornato { $count } di giorni fa
   *[other] aggiornato { $count } giorni fa
}

journal.window.shared.status.pane.updated.elapsed.hour = { $quantity ->
    [one] aggiornato { $count } ora fa
    [many] aggiornato { $count } di ore fa
   *[other] aggiornato { $count } ore fa
}

journal.window.shared.status.pane.updated.elapsed.minute = { $quantity ->
    [one] aggiornato { $count } minuto fa
    [many] aggiornato { $count } di minuti fa
   *[other] aggiornato { $count } minuti fa
}

journal.window.shared.status.pane.updated.elapsed.month = { $quantity ->
    [one] aggiornato { $count } mese fa
    [many] aggiornato { $count } di mesi fa
   *[other] aggiornato { $count } mesi fa
}

journal.window.shared.status.pane.updated.elapsed.second = { $quantity ->
    [one] aggiornato { $count } secondo fa
    [many] aggiornato { $count } di secondi fa
   *[other] aggiornato { $count } secondi fa
}

journal.window.shared.status.pane.updated.elapsed.week = { $quantity ->
    [one] aggiornato { $count } settimana fa
    [many] aggiornato { $count } di settimane fa
   *[other] aggiornato { $count } settimane fa
}

journal.window.shared.status.pane.version.current.up.to.date = v{ $current_version } · aggiornato

journal.window.shared.status.pane.version.package.repository.link = installato da un pacchetto. per ricevere gli aggiornamenti tramite il programma di aggiornamento del sistema, aggiungi il repository di solstone: { $repository_link }

journal.window.shared.status.pane.version.unknown.up.to.date = vsconosciuto · aggiornato

journal.window.shared.status.pane.version.update.available = v{ $current_version } · { $notice_start }aggiornamento disponibile (v{ $available_version }){ $notice_end }

journal.window.shared.status.pane.version.update.installed = v{ $current_version } · v{ $installed_version } è installata e si avvia la prossima volta che si avvia il tuo diario

journal.window.shared.status.pane.visible.text.a.security.update.is.installed.restart = un aggiornamento di sicurezza è installato. riavvia il tuo diario per completarlo.

journal.window.shared.status.pane.visible.text.couldn.t.check.for.security.updates = non è stato possibile cercare aggiornamenti di sicurezza.

journal.window.shared.status.pane.visible.text.device.status.is.unavailable.right.now = lo stato del dispositivo non è disponibile in questo momento.

journal.window.shared.status.pane.visible.text.devices.are.adding.to.your.journal = i dispositivi stanno aggiungendo al tuo diario

journal.window.shared.status.pane.visible.text.hide.details = nascondi i dettagli

journal.window.shared.status.pane.visible.text.just.now = proprio ora

journal.window.shared.status.pane.visible.text.no.devices.are.running.the.solstone = nessun dispositivo esegue ancora l'app di solstone. configurane uno per avviare il tuo diario.

journal.window.shared.status.pane.visible.text.no.messages.yet = ancora nessun messaggio

journal.window.shared.status.pane.visible.text.no.notifications.held.back = nessuna notifica trattenuta

journal.window.shared.status.pane.visible.text.no.notifications.yet = ancora nessuna notifica

journal.window.shared.status.pane.visible.text.nothing.held.back.nothing.new = niente di trattenuto, niente di nuovo

journal.window.shared.status.pane.visible.text.notifications = notifiche

journal.window.shared.status.pane.visible.text.quiet.notifications = notifiche silenziose

journal.window.shared.status.pane.visible.text.show.details = mostra i dettagli

journal.window.shared.status.pane.visible.text.solstone.app.install.linux.packages = solstone.app/install#linux-packages

journal.window.shared.status.pane.visible.text.under.a.minute = meno di un minuto

journal.window.shared.status.pane.visible.text.unknown = sconosciuto

journal.window.shared.status.pane.visible.text.update.status.unknown = stato dell'aggiornamento sconosciuto

journal.window.shared.status.pane.visible.text.view.device.health = vedi lo stato del dispositivo →

journal.window.shared.status.pane.window.explanation.a.device.hasn.t.added.to = un dispositivo non ha aggiunto al tuo diario di recente.

journal.window.shared.status.pane.window.explanation.a.device.needs.attention = un dispositivo richiede attenzione

journal.window.shared.status.pane.window.explanation.installed.from.a.package.updates.come = installato da un pacchetto. gli aggiornamenti arrivano tramite il programma di aggiornamento del sistema.

journal.window.shared.status.pane.window.explanation.installs.on.a.connection.that.isn = si installa su una connessione che non è a consumo

journal.window.shared.status.pane.window.explanation.once.a.day.your.journal.and = una volta al giorno il tuo diario e l'app di solstone cercano una nuova versione e la installano in secondo piano. il tuo diario non si riavvia mai da solo.

journal.window.shared.status.pane.window.explanation.once.a.day.your.journal.checks = una volta al giorno il tuo diario cerca una nuova versione e la installa in secondo piano. non si riavvia mai da solo.

journal.window.shared.status.pane.window.explanation.update.your.journal.automatically = aggiornare il tuo diario automaticamente?

journal.window.shared.status.pane.window.explanation.what.it.adds.isn.t.reaching = ciò che aggiunge non arriva nel tuo diario.

journal.window.shared.websocket.label.a.device.hasn.t.added.recently = un dispositivo non ha aggiunto di recente

journal.window.shared.websocket.label.a.device.needs.attention = un dispositivo richiede attenzione

journal.window.shared.websocket.label.attention = attenzione

journal.window.shared.websocket.label.can.t.confirm = impossibile confermare

journal.window.shared.websocket.label.connecting = connessione in corso

journal.window.shared.websocket.label.disconnected = disconnesso

journal.window.shared.websocket.label.going.in = in ingresso

journal.window.shared.websocket.label.no.devices.connected = nessun dispositivo connesso

journal.window.shared.websocket.label.reconnect.now = Riconnetti ora

journal.window.shared.websocket.label.show.details = Mostra i dettagli

journal.window.shared.websocket.message.last.reconnect.attempt.failed = l'ultimo tentativo di riconnessione non è riuscito.

journal.window.shared.websocket.message.live.updates.connected = aggiornamenti in diretta connessi

journal.window.shared.websocket.message.reconnecting.some.features.may.be.delayed = riconnessione in corso. alcune funzioni possono essere in ritardo

journal.window.shared.websocket.reconnect.failed.history = l'ultimo tentativo di riconnessione non è riuscito. tentativi recenti: { $attempt_times }

journal.window.shared.websocket.reconnect.failed.no.history = l'ultimo tentativo di riconnessione non è riuscito. ancora nessun tentativo di riconnessione registrato.

journal.window.shared.websocket.title.connection.lost = connessione persa

journal.window.shared.websocket.title.reconnected = riconnesso

journal.window.shell.connecting = connessione in corso…

journal.window.shell.error.pinned.must.be.a.boolean = fissato deve essere un booleano

journal.window.shell.error.pins.request.must.be.an.object = la richiesta di fissaggio deve essere un oggetto

journal.window.shell.error.that.view.can.t.be.pinned = non è possibile fissare quella vista

journal.window.shell.error.the.pins.request.could.not.be.read = non è stato possibile leggere la richiesta di fissaggio

journal.window.shell.error.unknown.view = vista sconosciuta

journal.window.shell.error.view.must.be.a.string = la vista deve essere una stringa

journal.window.shell.error.your.settings.couldn.t.be.read = non è stato possibile leggere le impostazioni.

journal.window.shell.guard.authorization.revoked = non è stato possibile usare quel dispositivo accoppiato perché è stato revocato.

journal.window.shell.guard.authorization.revoked.detail = dispositivo accoppiato revocato

journal.window.shell.guard.authorization.unavailable.detail = autorizzazione del dispositivo accoppiato non disponibile

journal.window.shell.guard.authorization.unavailable.mac = non è stato possibile controllare quali dispositivi sono accoppiati al tuo diario. se continua a succedere, apri il terminale di amministrazione dal menu journal dell'app del diario, sul mac su cui il tuo diario è in esecuzione, ed esegui `{ $slot1 }`.

journal.window.shell.guard.authorization.unavailable.other = non è stato possibile controllare quali dispositivi sono accoppiati al tuo diario. se continua a succedere, esegui `{ $slot1 }` sul computer su cui il tuo diario è in esecuzione.

journal.window.shell.guard.form.denied = il tuo diario ha respinto questa richiesta perché non è stata inviata dalle pagine del tuo diario.

journal.window.shell.guard.form.detail = le richieste che modificano lo stato richiedono { $slot1 }

journal.window.shell.guard.host.denied = l'app web del tuo diario si può aprire solo su { $slot1 }, { $slot2 } o { $slot3 }, sul computer su cui è in esecuzione.

journal.window.shell.guard.host.detail = host non consentito

journal.window.shell.guard.origin.detail = richiesta cross-origin

journal.window.shell.guard.site.denied = l'app web del tuo diario ha respinto questa richiesta perché proveniva da un altro sito.

journal.window.shell.guard.site.detail = richiesta cross-site

journal.window.shell.last.message = ultimo messaggio:

journal.window.shell.loading = caricamento…

journal.window.shell.notifications.empty = ancora nessuna notifica

journal.window.shell.notifications.enable = attiva le notifiche del browser

journal.window.shell.notifications.label = notifiche

journal.window.shell.notifications.quiet = notifiche silenziose

journal.window.shell.notifications.quiet.explanation = errori che non sono stati mostrati come notifiche, da servizi in secondo piano.

journal.window.shell.notifications.recent = notifiche recenti

journal.window.shell.pin.error.invalid = non è stato possibile salvare quell'impostazione perché un valore non era valido.

journal.window.shell.pin.error.save = non è stato possibile salvare le tue impostazioni.

journal.window.shell.presentation.banner = PRESENT — Maiusc+P per uscire

journal.window.shell.redirect.instruction = Reindirizzamento automatico all'URL di destinazione: { $destination_link }. Se non succede, clicca il link.

journal.window.shell.redirect.title = Reindirizzamento...

journal.window.shell.settings.read.failed = non è stato possibile leggere le impostazioni.

journal.window.shell.skip.to.workspace = salta all'area di lavoro

journal.window.shell.state = stato:

journal.window.shell.status = stato

journal.window.shell.status.details = dettagli dello stato del diario

journal.window.shell.status.silent.devices = { $count }{ $slot1 }dispositivi non hanno aggiunto di recente

journal.window.shell.technical.details = dettagli tecnici

journal.window.shell.title = journal

journal.window.shell.update.link = come aggiornare il tuo diario

journal.window.shell.uptime = tempo di attività:

journal.window.shell.version = versione

journal.window.shell.version.unknown = vsconosciuto

journal.window.shell.views = viste

journal.window.view.activities.date.count = { $quantity ->
    [one] 1 attività
    [many] { $count } di attività
   *[other] { $count } attività
}

journal.window.view.activities.date.none = nessuna attività

journal.window.view.activities.date.one = attività

journal.window.view.activities.date.other = attività

journal.window.view.activities.description = cosa hai fatto ogni giorno, ordinata per facetta

journal.window.view.activities.label = attività

journal.window.view.activities.source = resoconto del tuo diario da ciò che hai condiviso

journal.window.view.agents.description = connetti un agente per leggere e cercare nel tuo diario, entro ciò che scegli

journal.window.view.agents.label = agenti

journal.window.view.agents.source = agenti che connetti

journal.window.view.backup.description = una copia cifrata del tuo diario che solo tu puoi leggere

journal.window.view.backup.label = backup

journal.window.view.backup.source = il tuo spazio di archiviazione, o quello che { $slot1 } gestisce

journal.window.view.body.date.count = { $quantity ->
    [one] 1 lettura
    [many] { $count } di letture
   *[other] { $count } letture
}

journal.window.view.body.date.none = nessuna lettura

journal.window.view.body.date.one = lettura

journal.window.view.body.date.other = letture

journal.window.view.body.description = sonno, allenamenti e letture cardiache

journal.window.view.body.label = corpo

journal.window.view.body.source = da ciò che importi

journal.window.view.curation.description = modifiche che il tuo diario suggerisce, da accettare o saltare

journal.window.view.curation.label = suggerimenti

journal.window.view.curation.source = suggeriti dal tuo diario mentre lavora

journal.window.view.entities.description = le persone, le aziende, i progetti e gli strumenti che il tuo diario tiene, da modificare, unire o nascondere

journal.window.view.entities.label = entità

journal.window.view.entities.source = nomi che il tuo diario ha trovato in ciò che tiene

journal.window.view.health.description = se il tuo diario sta al passo: elaborazione, dispositivi, errori

journal.window.view.health.label = stato

journal.window.view.health.source = i controlli del tuo diario

journal.window.view.home.description = oggi e ieri, in resoconto, e tutto ciò di cui il tuo diario ha bisogno da te

journal.window.view.home.label = inizio

journal.window.view.home.source = scritto dal tuo diario da tutto ciò che contiene

journal.window.view.import.description = importa chat, calendari, note, audio e allenamenti

journal.window.view.import.label = importazione

journal.window.view.import.source = file che importi

journal.window.view.map.date.count = { $quantity ->
    [one] 1 registro
    [many] { $count } di registri
   *[other] { $count } registri
}

journal.window.view.map.date.none = nessuna posizione

journal.window.view.map.date.one = registro

journal.window.view.map.date.other = registri

journal.window.view.map.description = dove eri, giorno per giorno, e i luoghi che nomini

journal.window.view.map.label = mappa

journal.window.view.map.source = dall'app di solstone sul tuo telefono, una volta che la posizione è attiva lì

journal.window.view.more.label = altro

journal.window.view.network.description = accoppia un telefono o un computer, e come i tuoi dispositivi si connettono al tuo diario

journal.window.view.network.label = rete

journal.window.view.network.source = i tuoi dispositivi

journal.window.view.search.description = trova parole e frasi ovunque nel tuo diario

journal.window.view.search.label = ricerca

journal.window.view.search.source = tutto ciò che il tuo diario tiene

journal.window.view.settings.description = il tuo profilo, le chiavi, la trascrizione, lo spazio di archiviazione e le facette

journal.window.view.settings.label = impostazioni

journal.window.view.sources.description = quando i tuoi dispositivi hanno aggiunto qualcosa al tuo diario l'ultima volta.

journal.window.view.sources.label = fonti

journal.window.view.sources.source = i tuoi dispositivi accoppiati

journal.window.view.speakers.date.count = { $quantity ->
    [one] 1 segmento
    [many] { $count } di segmenti
   *[other] { $count } segmenti
}

journal.window.view.speakers.date.none = nessun segmento

journal.window.view.speakers.date.one = segmento

journal.window.view.speakers.date.other = segmenti

journal.window.view.speakers.description = le voci nel tuo audio: insegna la tua, nomina le altre, correggi chi ha detto cosa

journal.window.view.speakers.label = parlanti

journal.window.view.speakers.source = voci nell'audio che hai condiviso

journal.window.view.stats.description = quanto tiene il tuo diario, e quanta elaborazione ha fatto

journal.window.view.stats.label = statistiche

journal.window.view.stats.source = i conteggi del tuo diario

journal.window.view.support.description = crea una segnalazione, o seguine una che hai creata

journal.window.view.support.label = supporto

journal.window.view.thinking.description = dove avviene il ragionamento, e ogni esecuzione

journal.window.view.thinking.label = ragionamento

journal.window.view.thinking.source = sul computer del tuo diario, sull'elaborazione confidenziale gestita da { $slot1 }, o con il tuo modello

journal.window.view.transcripts.date.count = { $quantity ->
    [one] 1 segmento
    [many] { $count } di segmenti
   *[other] { $count } segmenti
}

journal.window.view.transcripts.date.none = nessun segmento

journal.window.view.transcripts.date.one = segmento

journal.window.view.transcripts.date.other = segmenti

journal.window.view.transcripts.description = ciò che è stato detto e ciò che c'era sullo schermo, momento per momento, con l'audio

journal.window.view.transcripts.label = trascrizioni

journal.window.view.transcripts.source = dal tuo microfono e dallo schermo tramite l'app di solstone, e dalle importazioni

journal.window.view.year.description = un anno intero tutto in una volta: ogni giorno, e ogni luogo e nome per settimana

journal.window.view.year.label = anno

journal.window.view.year.source = contato da ciò che il tuo diario tiene

journal.window.year.copy.days.days = giorni

journal.window.year.copy.days.lede.each.square.is.a.day.colored = ogni casella è un giorno, colorata in base al conteggio che scegli sotto.

journal.window.year.copy.empty.action.see.your.sources = vedi le tue fonti

journal.window.year.copy.empty.detail.once.your.journal.has.activities.places = una volta che il tuo diario ha attività, luoghi o nomi per un giorno, il suo anno compare qui.

journal.window.year.copy.empty.nothing.here.yet = qui non c'è ancora niente.

journal.window.year.copy.error.this.didn.t.load = questo non si è caricato.

journal.window.year.copy.find.find.a.place.or.a.name = trova un luogo o un nome

journal.window.year.copy.layer.label.what.to.count = cosa contare

journal.window.year.copy.names.unavailable.names.aren.t.listed.your.journal = i nomi non sono elencati: non è stato possibile controllare quali lasciare fuori.

journal.window.year.copy.other.names.other.names = altri nomi

journal.window.year.copy.peek.hint.choose.a.row.or.a.week = scegli una riga, o una settimana su di essa, per vederne i giorni.

journal.window.year.copy.places.places = luoghi

journal.window.year.copy.places.unavailable.places.aren.t.listed.the.place = i luoghi non sono elencati: non è stato possibile leggere i nomi dei luoghi forniti con il tuo diario.

journal.window.year.copy.retry.try.again = riprova

journal.window.year.copy.step.label.other.years = altri anni

journal.window.year.copy.switch.label.how.to.see.this.year = come vedere questo anno

journal.window.year.copy.threads.lede.each.row.is.a.place.or = ogni riga è un luogo o un nome trovato dal tuo diario. ogni casella è una settimana, colorata per quanti giorni di quella settimana vi compare il luogo o il nome. le righe sono in ordine di prima comparsa.

journal.window.year.copy.threads.threads = fili

journal.window.year.copy.title.year = anno

journal.window.year.copy.week.after.the.later.week.with.days.on = la settimana più avanti con giorni su questa riga

journal.window.year.copy.week.before.the.earlier.week.with.days.on = la settimana più indietro con giorni su questa riga

journal.window.year.days.empty = niente di questo tipo nel { $year }.

journal.window.year.months.label.apr = Apr

journal.window.year.months.label.aug = Ago

journal.window.year.months.label.dec = Dic

journal.window.year.months.label.feb = Feb

journal.window.year.months.label.jan = Gen

journal.window.year.months.label.jul = Lug

journal.window.year.months.label.jun = Giu

journal.window.year.months.label.mar = Mar

journal.window.year.months.label.may = Mag

journal.window.year.months.label.nov = Nov

journal.window.year.months.label.oct = Ott

journal.window.year.months.label.sep = Set

journal.window.year.source.or.date.unit.label.activities = attività

journal.window.year.source.or.date.unit.label.activity = attività

journal.window.year.source.or.date.unit.label.location = posizione

journal.window.year.source.or.date.unit.label.no.activities = nessuna attività

journal.window.year.source.or.date.unit.label.no.location = nessuna posizione

journal.window.year.source.or.date.unit.label.record = registro

journal.window.year.source.or.date.unit.label.records = registri

journal.window.year.thread.cell.week.days = { $days_quantity ->
    [one] { $thread_name }, settimana del { $week_start }: { $days_count } giorno
    [many] { $thread_name }, settimana del { $week_start }: { $days_count } di giorni
   *[other] { $thread_name }, settimana del { $week_start }: { $days_count } giorni
}

journal.window.year.thread.days.in.year = { $days_quantity ->
    [one] 1 giorno nel { $year }, { $first_day }.
    [many] { $days_count } di giorni nel { $year }, dal { $first_day } al { $last_day }, nell'arco di { $weeks_quantity ->
        [one] { $weeks_count } settimana.
        [many] { $weeks_count } di settimane.
       *[other] { $weeks_count } settimane.
    }
   *[other] { $days_count } giorni nel { $year }, dal { $first_day } al { $last_day }, nell'arco di { $weeks_quantity ->
        [one] { $weeks_count } settimana.
        [many] { $weeks_count } di settimane.
       *[other] { $weeks_count } settimane.
    }
}

journal.window.year.thread.empty = nessun luogo o nome nel { $year }.

journal.window.year.thread.find.empty = niente nel { $year } ha quel nome.

journal.window.year.thread.group.name.count = { $total_quantity ->
    [one] { $total_count } nome
    [many] { $total_count } di nomi
   *[other] { $total_count } nomi
}

journal.window.year.thread.group.place.count = { $total_quantity ->
    [one] { $total_count } luogo
    [many] { $total_count } di luoghi
   *[other] { $total_count } luoghi
}

journal.window.year.thread.jump.to.first.day = inizia da { $first_day }

journal.window.year.thread.list.empty = niente in elenco per il { $year }.

journal.window.year.thread.week.day.links = { $days_quantity ->
    [one] { $days_count } giorno di questa settimana: { $day_links }
    [many] { $days_count } di giorni di questa settimana: { $day_links }
   *[other] { $days_count } giorni di questa settimana: { $day_links }
}

journal.window.year.thread.week.range = settimana dal { $start_day } al { $end_day }

journal.window.year.type.labels.company.companies = aziende

journal.window.year.type.labels.organization.organizations = organizzazioni

journal.window.year.type.labels.person.people = persone

journal.window.year.type.labels.place.named.places = luoghi nominati

journal.window.year.type.labels.project.projects = progetti

journal.window.year.type.labels.tool.tools = strumenti

journal.window.year.view.empty = niente da mostrare qui per il { $year }.

journal.window.year.year.invalid = non è stato possibile usare quell'anno.

journal.window.year.year.invalid.detail = Anno non valido, previsto YYYY
