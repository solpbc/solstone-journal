journal.mac.admin.terminal.failure.title = non è stato possibile aprire il terminale di amministrazione

journal.mac.admin.terminal.menu.item.title = apri il terminale di amministrazione

journal.mac.app.journal = journal

journal.mac.backup.setup.hint = il backup tiene al sicuro il tuo diario. configuralo dal tuo diario.

journal.mac.connection.invalid.response = l'app del diario non è riuscita a leggere ciò che è tornato dal tuo diario.

journal.mac.connection.link.failed = l'app del diario non è riuscita ad aprire il tuo diario nel tuo browser. riprova.

journal.mac.connection.refused = l'app del diario non è riuscita a connettersi al tuo diario su questo mac.

journal.mac.connection.timed.out = l'app del diario ha atteso troppo a lungo il tuo diario. riprova.

journal.mac.connection.unavailable = il tuo diario non è ancora pronto per l'app del diario. riprova tra poco.

journal.mac.devices.add.device = accoppia un dispositivo

journal.mac.devices.cancel = annulla

journal.mac.devices.close = chiudi

journal.mac.devices.copied = copiato ✓

journal.mac.devices.copy.link = copia il link

journal.mac.devices.detail.network = { $network } · { $activity }

journal.mac.devices.detail.network.paired = { $network } · { $activity } · accoppiato { $date }

journal.mac.devices.detail.paired = { $activity } · accoppiato { $date }

journal.mac.devices.empty.body = accoppia un dispositivo e entra nel tuo diario.

journal.mac.devices.empty.title = ancora nessun dispositivo

journal.mac.devices.expires.in = scade tra { $minutes }:{ $seconds }

journal.mac.devices.link.expired.body = apri un nuovo link per accoppiare un dispositivo.

journal.mac.devices.link.expired.title = link scaduto

journal.mac.devices.loading = caricamento dei dispositivi...

journal.mac.devices.model.last.seen = visto l'ultima volta { $time }

journal.mac.devices.model.last.seen.just.now = visto l'ultima volta proprio ora

journal.mac.devices.model.last.seen.unknown = visto l'ultima volta: sconosciuto

journal.mac.devices.model.never.connected = mai connesso

journal.mac.devices.model.status = stato { $status }

journal.mac.devices.not.ready.body = termina la configurazione iniziale del tuo diario, poi riprova.

journal.mac.devices.not.ready.title = i dispositivi non sono pronti

journal.mac.devices.not.running.body = avvia il diario, poi riprova.

journal.mac.devices.not.running.title = il diario non è in esecuzione

journal.mac.devices.open.fresh.link = apri un nuovo link

journal.mac.devices.paired.done = fatto

journal.mac.devices.paired.heading = "{ $label }" ora può connettersi al tuo diario

journal.mac.devices.paired.heading.unnamed = un nuovo dispositivo ora può connettersi al tuo diario

journal.mac.devices.paired.note = su quel dispositivo, controlla che mostri la stessa marca, poi rispondi lì. non l'hai accoppiato?

journal.mac.devices.paired.note.anywhere = su quel dispositivo, controlla che mostri la stessa marca, poi rispondi lì. può connettersi al tuo diario da qualsiasi luogo. non l'hai accoppiato?

journal.mac.devices.paired.remove.failed = non è stato possibile disaccoppiare questo dispositivo. riprova, oppure rimuovilo da dispositivi.

journal.mac.devices.paired.removed = disaccoppiato. il dispositivo non può connettersi al tuo diario finché non viene accoppiato di nuovo.

journal.mac.devices.paired.unpair = disaccoppia questo dispositivo

journal.mac.devices.pairing.code = codice di accoppiamento

journal.mac.devices.pairing.failed.title = non è stato possibile aprire l'accoppiamento

journal.mac.devices.pairing.instructions = scansiona questo codice oppure apri il link sul dispositivo che vuoi accoppiare.

journal.mac.devices.pairing.link.label = link di accoppiamento

journal.mac.devices.pairing.opening = apertura dell'accoppiamento...

journal.mac.devices.pairing.title = accoppia un dispositivo

journal.mac.devices.pane.title = dispositivi

journal.mac.devices.peer.journals.header = diari pari

journal.mac.devices.remove = rimuovi

journal.mac.devices.revoke.body = { $name } perde l'accesso al tuo diario. puoi accoppiarlo di nuovo più tardi.{ $space1 }{ $detail }

journal.mac.devices.revoke.failed = non è stato possibile rimuovere questo dispositivo

journal.mac.devices.revoke.title = rimuovi { $name }?

journal.mac.devices.try.again = riprova

journal.mac.devices.unnamed.device = dispositivo senza nome

journal.mac.devices.unnamed.journal = diario senza nome

journal.mac.devices.your.devices.header = i tuoi dispositivi

journal.mac.events.completed = configurazione iniziale ok

journal.mac.events.completed.duration = configurazione iniziale ok ({ $duration_ms }ms)

journal.mac.events.doctor.check = { $space0 }[{ $label }] { $name } - { $detail }

journal.mac.events.doctor.started = doctor avviato

journal.mac.events.doctor.status = doctor: { $status }

journal.mac.events.doctor.summary = { $total ->
    [one] doctor: { $total } controllo ({ $failed } falliti, { $warnings } avvisi, { $skipped } saltati)
    [many] doctor: { $total } di controlli ({ $failed } falliti, { $warnings } avvisi, { $skipped } saltati)
   *[other] doctor: { $total } controlli ({ $failed } falliti, { $warnings } avvisi, { $skipped } saltati)
}

journal.mac.events.done = passo { $step } fatto ({ $outcome })

journal.mac.events.done.duration = passo { $step } fatto ({ $outcome }, { $duration_ms }ms)

journal.mac.events.fail = errore

journal.mac.events.failed = configurazione iniziale non riuscita

journal.mac.events.failed.step = configurazione iniziale non riuscita a { $failed_step }

journal.mac.events.malformed = [evento malformato] { $raw }

journal.mac.events.ok = ok

journal.mac.events.skip = saltato

journal.mac.events.skipped = passo { $step } saltato

journal.mac.events.skipped.reason = passo { $step } saltato ({ $reason })

journal.mac.events.started = configurazione iniziale avviata

journal.mac.events.started.version = configurazione iniziale avviata (versione { $version }, { $mode })

journal.mac.events.step = passo: { $step }

journal.mac.events.step.failed = passo { $step } non riuscito: { $message }

journal.mac.events.step.indexed = passo { $index }/{ $total }: { $step }

journal.mac.events.unknown = sconosciuto

journal.mac.events.unparseable = [riga non analizzabile] { $raw }

journal.mac.events.unrecognized = [evento non riconosciuto] { $raw }

journal.mac.events.warn = avviso

journal.mac.events.warning = { $space0 }avviso: { $text }

journal.mac.events.warning.fix = { $space0 }avviso: { $text } (correzione: { $fix_hint })

journal.mac.first.run.model.setup.did.not.finish = configurazione iniziale non terminata

journal.mac.first.run.model.setup.reported.completion.twice = la configurazione iniziale ha segnalato il completamento due volte

journal.mac.first.run.model.setup.timed.out = tempo scaduto per la configurazione iniziale

journal.mac.health.check.denied = negato

journal.mac.health.check.granted = concesso

journal.mac.health.check.microphone = microfono

journal.mac.health.check.screen.recording = registrazione dello schermo

journal.mac.health.check.unknown.checking.permissions = sconosciuto, controllo dei permessi

journal.mac.health.microphone.permission.help = apri Impostazioni di Sistema › Privacy e sicurezza e consenti Microfono per { $product }

journal.mac.health.screen.permission.help = apri Impostazioni di Sistema › Privacy e sicurezza e consenti Registrazione dello schermo e dell'audio di sistema per { $product }

journal.mac.home.browser.explanation = questa app tiene il tuo diario in esecuzione su questo mac. il tuo diario stesso si apre nel tuo browser.

journal.mac.home.unconfigured = qui non c'è ancora niente. la creazione del tuo diario viene dopo.

journal.mac.info.cfbundle.display.name = journal

journal.mac.info.cfbundle.name = journal

journal.mac.info.nshuman.readable.copyright = © 2026 { $company }. tutti i diritti riservati.

journal.mac.local.endpoint.unavailable = il tuo diario non è ancora pronto per l'app del diario. riprova tra poco.

journal.mac.location.cancel = annulla

journal.mac.location.leave.setup.confirm = la configurazione iniziale si ferma.

journal.mac.location.new.journal.hosted.share = se ripristini lì il backup di questo diario, il nuovo diario lo prende in carico, e questo smette di fare il backup finché non lo attivi di nuovo.

journal.mac.location.other.journals = altri diari su questo mac

journal.mac.location.same.mark.template = entrambi hanno la stessa marca, così i tuoi dispositivi si connettono a quello che è in esecuzione.

journal.mac.location.shared.hosted.template = quello in { $path } condivide questo backup cifrato operato da { $company }, e solo uno può usarlo. se questo diario smette di fare il backup, attivalo di nuovo nella sezione backup.

journal.mac.location.start.new.journal = avvia un nuovo diario

journal.mac.location.start.new.journal.confirm.template = il tuo diario in { $path } si ferma e resta lì, invariato. poi inizia la configurazione iniziale, dove puoi ripristinare da un backup.

journal.mac.location.start.new.journal.confirm.title = avvia un nuovo diario?

journal.mac.location.start.new.journal.help = il tuo diario si ferma e resta nella sua cartella, invariato. durante la configurazione iniziale puoi ripristinare da un backup, e puoi tornare a questo qui.

journal.mac.location.switch.button = passa

journal.mac.location.switch.confirm.template = il tuo diario in { $path } si ferma e resta lì, invariato. puoi tornare a questo qui.

journal.mac.location.switch.confirm.title.template = passa al diario in { $path }?

journal.mac.location.switch.to.journal = passa a questo diario

journal.mac.restart.journal.path.unreadable = riavvio non riuscito: non è stato possibile leggere il percorso del diario

journal.mac.restart.restart.failed.journal.did.not.come.back = riavvio non riuscito: il diario non è tornato

journal.mac.restart.restart.failed.journal.did.not.restart = riavvio non riuscito: il diario non si è riavviato

journal.mac.runtime.bundle.missing = l'ambiente di esecuzione nativo incluso del diario manca in { $path }

journal.mac.runtime.child.admission.failed = non è stato possibile verificare l'ammissione del processo figlio del diario

journal.mac.runtime.child.containment.pending = il contenimento del processo figlio del diario è in corso

journal.mac.runtime.child.unconfigured = il processo figlio del diario non è configurato

journal.mac.runtime.couldn.t.get.ready.continuing = non è stato possibile preparare { $library }; si continua

journal.mac.runtime.executable.missing = l'eseguibile nativo incluso di { $product } manca in { $path }

journal.mac.runtime.installer.readiness.gate.failed = non è stato possibile preparare il diario per questo mac

journal.mac.runtime.journal.child.breaker.tripped = il tuo diario si è fermato inaspettatamente più volte entro un minuto

journal.mac.runtime.journal.child.containment.unresolved = il tuo diario si è fermato, e non è stato possibile confermare la chiusura di tutti i suoi processi

journal.mac.runtime.journal.drive.cannot.run = il tuo diario non può funzionare su questo disco perché il disco non può salvare i file come serve al tuo diario. usa un disco formattato per mac per il tuo diario.

journal.mac.runtime.journal.materialize.failed = non è stato possibile preparare l'ambiente di esecuzione del diario

journal.mac.runtime.journal.readiness.timeout = il diario non è diventato pronto in tempo

journal.mac.runtime.journal.setup.needed.before.upgrade = è necessaria la configurazione iniziale del diario prima che l'aggiornamento possa continuare

journal.mac.runtime.journal.spawn.blocked.legacy.service = qualcos'altro è configurato per eseguire il tuo diario

journal.mac.runtime.journal.spawn.blocked.ports = le porte del diario sono ancora in uso

journal.mac.runtime.journal.spawn.port.check.failed = non è stato possibile verificare che le porte del diario siano libere

journal.mac.runtime.journal.spawn.port.held.elsewhere = il tuo diario non può avviarsi perché un'altra app sta usando la connessione che gli serve su questo mac. su un mac condiviso, di solito è il diario di un altro account.

journal.mac.runtime.journal.spawn.service.check.failed = non è stato possibile stabilire se qualcos'altro stia eseguendo il tuo diario

journal.mac.runtime.release.identity.missing = a journal.app manca un'identità di versione valida

journal.mac.runtime.required.models.failed = { $command } non riuscito (uscita { $exit_code }, terminazione { $reason })

journal.mac.settings.backup = backup

journal.mac.settings.copied = copiato

journal.mac.settings.copy = copia

journal.mac.settings.couldn.t.copy.select.the.text.and.copy.it = non è stato possibile copiare. seleziona il testo e copialo.

journal.mac.settings.disk.used = spazio occupato

journal.mac.settings.health = stato

journal.mac.settings.launch.the.journal.when.you.log.in = avvia il diario quando accedi

journal.mac.settings.location = posizione

journal.mac.settings.open.backup = apri il backup

journal.mac.settings.open.your.journal = apri il tuo diario

journal.mac.settings.restart = riavvia

journal.mac.settings.run.state = stato di esecuzione

journal.mac.settings.run.state.link = stato di esecuzione →

journal.mac.settings.start = avvia

journal.mac.settings.startup = avvio

journal.mac.settings.stop = ferma

journal.mac.settings.the.journal.app.is.available = l'app del diario { $version } è disponibile.

journal.mac.settings.the.journal.update.check.failed = la ricerca di aggiornamenti del diario non è riuscita.

journal.mac.settings.update.failed.with.version = la ricerca di aggiornamenti non è riuscita. l'app del diario { $version } era stata trovata prima.

journal.mac.settings.update.needs.attention = l'aggiornamento richiede attenzione

journal.mac.settings.view.journal.updates = vedi gli aggiornamenti del diario

journal.mac.settings.your.journal.at.a.glance = il tuo diario, a colpo d'occhio

journal.mac.settings.your.journal.mark.not.read.yet = il tuo diario, marca non ancora letta

journal.mac.setup.adopt.failed = non è stato possibile aprire questo diario

journal.mac.setup.adopt.landing.line = non si è spostato niente. il tuo diario è sempre stato qui. lo riconosci dalla sua marca.

journal.mac.setup.adopt.opening = apertura di questo diario

journal.mac.setup.adopt.title = diario trovato

journal.mac.setup.back = indietro

journal.mac.setup.choose.location = scegli

journal.mac.setup.chrome.of = { $done } di { $total }

journal.mac.setup.continue.button = continua

journal.mac.setup.continue.in.the.solstone.app = continua nell'app di { $product }

journal.mac.setup.download.bytes = { $done } / { $total }

journal.mac.setup.finish.in.browser.line = questa finestra va avanti quando finisci.

journal.mac.setup.finish.in.browser.title = termina la configurazione iniziale nel tuo browser

journal.mac.setup.folder.holds.another.journal = questa cartella si sovrappone a un altro diario su questo mac. scegli una cartella diversa.

journal.mac.setup.go.back.to.the.journal.in = torna al diario in { $path }

journal.mac.setup.handoff.competing.setup = l'app di { $product } sta già terminando un'altra configurazione iniziale. terminala lì, oppure apri questo diario qui.

journal.mac.setup.handoff.held.pairing = l'app di { $product } è già connessa a un diario. apri l'app di { $product } per cambiare quella connessione, oppure apri questo diario qui.

journal.mac.setup.handoff.not.delivered = non è stato possibile continuare la configurazione iniziale nell'app di { $product }.

journal.mac.setup.handoff.pending = la configurazione iniziale non è continuata nell'app di { $product }. aprila per continuare, oppure riprova qui.

journal.mac.setup.journal.folder = cartella del diario

journal.mac.setup.location.field = posizione

journal.mac.setup.login.item.change.failed = non è stato possibile cambiare l'avvio all'accesso.

journal.mac.setup.login.item.unavailable = l'avvio all'accesso non è disponibile da questa posizione dell'app.

journal.mac.setup.name.location.title = creiamo il tuo diario su questo mac

journal.mac.setup.on.this.mac = su questo mac

journal.mac.setup.on.this.mac.description = le richieste al modello restano su questo mac. continua avvia lo scaricamento.

journal.mac.setup.open.your.journal = apri il tuo diario

journal.mac.setup.reopen.in.browser.button = apri la pagina di configurazione iniziale

journal.mac.setup.restore.from.backup = ripristina da un backup

journal.mac.setup.runner.solstone.journal.setup.failed = { $setup_command } non riuscito

journal.mac.setup.set.up.solstone = configura { $product }

journal.mac.setup.setup.progress.not.saved = non è stato possibile salvare l'avanzamento della configurazione iniziale.

journal.mac.setup.setup.subtitle = può volerci un minuto.

journal.mac.setup.setup.title = configurazione del tuo diario in corso

journal.mac.setup.start.automatically.help = avvia automaticamente dopo che accedi a questo mac.

journal.mac.setup.start.setup = avvia la configurazione iniziale

journal.mac.setup.step.progress = passo { $step } di 7

journal.mac.setup.the.solstone.app = l'app di { $product }

journal.mac.setup.the.solstone.app.description = funziona su questo mac. il tuo microfono, lo schermo e il suono del computer restano disattivi finché non li attivi.

journal.mac.setup.thinking = ragionamento

journal.mac.setup.thinking.description = aiuta il tuo diario a dare un senso a ciò che tiene. scegli dove, durante la configurazione iniziale o più tardi.

journal.mac.setup.thinking.failed = non è stato possibile terminare questo passo. riprova.

journal.mac.setup.try.again = riprova

journal.mac.setup.what.this.sets.up = che cosa configura

journal.mac.setup.what.this.sets.up.journal = si trova su questo mac e tiene tutto ciò che vi entra.

journal.mac.setup.where.does.thinking.happen = dove avviene il ragionamento?

journal.mac.setup.where.does.thinking.happen.description = il tuo diario resta su questo mac qualunque cosa tu scelga. questo decide solo dove avviene il ragionamento.

journal.mac.setup.where.your.journal.lives = dove si trova il tuo diario

journal.mac.setup.where.your.journal.lives.description = tutto ciò che il tuo diario tiene resta in questa cartella, su questo mac.

journal.mac.setup.your.journal = il tuo diario

journal.mac.updates.deferred = l'aggiornamento del tuo diario continuerà dopo che il tuo diario è pronto.

journal.mac.updates.ready.to.install = l'app del diario { $version } è scaricata e pronta per installare.

journal.mac.window.model.backup = backup

journal.mac.window.model.blocked = bloccato

journal.mac.window.model.devices = dispositivi

journal.mac.window.model.healthy = in buono stato

journal.mac.window.model.home = inizio

journal.mac.window.model.journal = journal

journal.mac.window.model.location = posizione

journal.mac.window.model.not.set = non impostato

journal.mac.window.model.run.state = stato di esecuzione

journal.mac.window.model.running = in esecuzione

journal.mac.window.model.starting = avvio…

journal.mac.window.model.startup = avvio

journal.mac.window.model.stopped = fermato

journal.mac.window.model.unknown = sconosciuto

journal.mac.window.model.updates = aggiornamenti
