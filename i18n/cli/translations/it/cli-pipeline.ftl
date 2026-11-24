journal.cli.backup.config.canonical.recovery.characters = la chiave di ripristino canonica contiene caratteri { $Crockford } non validi

journal.cli.backup.config.canonical.recovery.length = la chiave di ripristino canonica deve essere lunga esattamente 64 caratteri

journal.cli.backup.config.entropy = non è stato possibile generare la chiave di backup.

journal.cli.backup.config.hosted.delete = non è stato possibile eliminare il collegamento ospitato.

journal.cli.backup.config.hosted.read = non è stato possibile leggere il collegamento ospitato.

journal.cli.backup.config.hosted.write = non è stato possibile salvare il collegamento ospitato.

journal.cli.backup.config.invalid.destination.shape = la destinazione del backup deve essere un oggetto { $JSON }

journal.cli.backup.config.invalid.mode = la modalità del backup deve essere { $byo } o { $operated }

journal.cli.backup.config.invalid.offload.bytes = i valori in byte del trasferimento del backup devono essere interi positivi o null

journal.cli.backup.config.invalid.offload.enabled = il valore enabled del trasferimento del backup deve essere un booleano

journal.cli.backup.config.invalid.offload.shape = il trasferimento del backup deve includere { $enabled }, { $budget_bytes }, { $floor_bytes }

journal.cli.backup.config.invalid.offload.status = lo stato del trasferimento del backup deve essere { $ok_value }, { $skipped_value }, { $stalled_value }, o { $error_value }

journal.cli.backup.config.invalid.restore.counters = i contatori del ripristino del backup devono essere interi non negativi

journal.cli.backup.config.invalid.restore.scope = l'ambito del ripristino del backup deve essere { $day_value }, { $all_value }, o { $journal_value }

journal.cli.backup.config.invalid.restore.status = lo stato del ripristino del backup deve essere { $ok_value }, { $no_op_value }, { $refused_value }, { $degraded_value }, o { $error_value }

journal.cli.backup.config.invalid.retention.shape = la conservazione del backup deve includere { $hourly }, { $daily }, { $weekly }, { $monthly }

journal.cli.backup.config.invalid.retention.value = i valori di conservazione del backup devono essere interi non negativi

journal.cli.backup.config.invalid.verification.status = lo stato di verifica del backup deve essere { $ok_value }, { $skipped_value }, o { $error_value }

journal.cli.backup.config.lock.timeout = non è stato possibile acquisire il blocco per { $path } entro { $seconds } s

journal.cli.backup.config.missing.credential = manca la credenziale di backup: { $key }

journal.cli.backup.config.recovery.parse = la chiave di ripristino deve contenere esattamente 64 caratteri { $Crockford } dopo la pulizia

journal.cli.backup.config.stored.keys = le chiavi del backup devono essere stringhe se presenti

journal.cli.backup.config.teardown.recovery.corrupt = il registro di ripristino dello smantellamento è corrotto o non valido.

journal.cli.backup.config.teardown.recovery.delete = non è stato possibile eliminare il registro di ripristino dello smantellamento.

journal.cli.backup.config.teardown.recovery.read = non è stato possibile leggere il registro di ripristino dello smantellamento.

journal.cli.backup.config.teardown.recovery.write = non è stato possibile salvare il registro di ripristino dello smantellamento.

journal.cli.backup.config.unsupported.backend = backend di backup non supportato

journal.cli.backup.destination.backend.prefix = Il repository deve iniziare con { $backend }: (per esempio { $example }).

journal.cli.backup.destination.hosted = { $company } gestisce lo spazio di archiviazione del tuo backup, quindi non c'è una destinazione da cambiare qui. gestiscilo nel portale dei servizi.

journal.cli.backup.destination.refused.auth.failed = la destinazione ha rifiutato le credenziali. non è stato salvato niente.

journal.cli.backup.destination.refused.bucket.not.found = queste credenziali non possono aprire un bucket con quel nome. controlla il nome del bucket nel repository. non è stato salvato niente.

journal.cli.backup.destination.refused.locked = il backup lì è occupato. riprova tra poco. non è stato salvato niente.

journal.cli.backup.destination.refused.other = non è stato possibile aprire la destinazione. controlla il repository e le credenziali, poi riprova. non è stato salvato niente.

journal.cli.backup.destination.refused.repo.missing = non è stato trovato nessun backup lì con queste credenziali. non è stato salvato niente.

journal.cli.backup.destination.refused.repository.key.refused = c'è già un backup lì che la tua chiave di ripristino non può aprire. scegli un'altra destinazione. non è stato salvato niente.

journal.cli.backup.destination.refused.timeout = la destinazione non ha risposto in tempo. controlla il repository e le credenziali, poi riprova. non è stato salvato niente.

journal.cli.backup.destination.setup.failed = non è stato possibile configurare un backup lì ({ $reason }). non è stato salvato niente.

journal.cli.backup.destination.switch.confirm = per passare, esegui di nuovo con { $confirm_new_repository_flag }.

journal.cli.backup.destination.switch.lead = è un repository diverso. il tuo backup esistente resta dov'è, intatto, e il tuo diario non farà il backup su di esso né il ripristino da esso.

journal.cli.backup.enable.complete = Backup attivato.

journal.cli.backup.enable.complete.key = Backup attivato. La tua chiave di ripristino:{ $line_break }{ $key_grid }

journal.cli.backup.enable.confirm.key = La tua chiave di ripristino (annotala - è l'unico modo per ripristinare):{ $line_break }{ $key_grid }{ $line_break }Conferma rimandando la chiave in pipe: { $command_enable }

journal.cli.backup.enable.destination.required = Imposta prima una destinazione: { $command_destination_set }

journal.cli.backup.enable.key.mismatch = La chiave di ripristino non corrispondeva.

journal.cli.backup.enable.repository.missing = Repository non trovato; preinizializzalo o imposta { $recovery_setting } nella configurazione.

journal.cli.backup.error = Errore: { $message }

journal.cli.backup.help.destination = uso: { $syntax }

journal.cli.backup.help.offload = uso: { $syntax }

journal.cli.backup.help.recovery.key = uso: { $syntax }

journal.cli.backup.help.root = uso: { $syntax }

journal.cli.backup.hosted.field = Manca { $name }.

journal.cli.backup.init.code = L'inizializzazione del repository non è riuscita ({ $code_field }{ $returncode }).

journal.cli.backup.init.failed = l'inizializzazione del repository non è riuscita.

journal.cli.backup.init.retained = l'inizializzazione del repository non è riuscita; lo stato conservato dell'operazione richiede una revisione.

journal.cli.backup.input.io = { $stdin_stream }: errore di I/O.

journal.cli.backup.input.json = { $json_format } non valido su { $stdin_stream }: { $error }

journal.cli.backup.input.limit = la richiesta di { $stdin_stream } supera il limite di input { $json_format }.

journal.cli.backup.input.object = atteso un oggetto { $json_format } su { $stdin_stream }.

journal.cli.backup.offload.media.byte.overflow = overflow del totale dei byte dei contenuti multimediali

journal.cli.backup.offload.media.file.overflow = overflow del totale dei file dei contenuti multimediali

journal.cli.backup.offload.preview.held = un'anteprima di trasferimento in sola lettura non è disponibile. non è stato avviato nessun lavoro di trasferimento.

journal.cli.backup.offload.restore.day.invalid = Giorno non valido.

journal.cli.backup.offload.restore.day.or.all = Usa un giorno o { $all_flag }, non entrambi.

journal.cli.backup.offload.restore.day.required = Fornisci un giorno o { $all_flag }.

journal.cli.backup.offload.restore.heading = ripristino del trasferimento del backup:

journal.cli.backup.offload.restore.reason.archive.unbound = il registro del backup non identifica l'archivio compresso necessario per ripristinare questi file

journal.cli.backup.offload.restore.reason.backup.not.ready = il tuo backup non è configurato, quindi i file trasferiti non si possono riportare indietro

journal.cli.backup.offload.restore.reason.destination.admission.failed = non è stato possibile connettersi alla destinazione del backup

journal.cli.backup.offload.restore.reason.failed = il tentativo di ripristino non è riuscito per un motivo non specificato

journal.cli.backup.offload.restore.reason.insufficient.free.space = non c'è abbastanza spazio libero su questo computer per riportare i file

journal.cli.backup.offload.restore.reason.ledger.degraded = il registro di ciò che è stato trasferito è danneggiato e non si può usare per il ripristino

journal.cli.backup.offload.restore.reason.legacy.read.failed = non è stato possibile leggere la copia di backup dei contenuti multimediali trasferiti da una versione precedente del tuo diario, quindi non è stata ripristinata

journal.cli.backup.offload.restore.reason.legacy.review.held = non è stato possibile controllare i contenuti multimediali trasferiti da una versione precedente del tuo diario rispetto al tuo backup, quindi non sono stati ripristinati

journal.cli.backup.offload.restore.reason.legacy.verification.failed = la copia di backup dei contenuti multimediali trasferiti da una versione precedente del tuo diario non corrispondeva al suo registro, quindi non è stata ripristinata

journal.cli.backup.offload.restore.reason.locked = il backup alla destinazione è bloccato, di solito da un'esecuzione ancora in corso o da una che è stata interrotta

journal.cli.backup.offload.restore.reason.missing.file.after.restore = manca un file che sarebbe dovuto tornare con il ripristino

journal.cli.backup.offload.restore.reason.nothing.to.restore = non c'era niente di trasferito da ripristinare

journal.cli.backup.offload.restore.reason.rclone.unavailable = il programma { $rclone_program } non è disponibile

journal.cli.backup.offload.restore.reason.repo.missing = non c'è ancora un backup alla destinazione

journal.cli.backup.offload.restore.reason.restic.unavailable = il programma di backup { $restic_program } non è disponibile

journal.cli.backup.offload.restore.reason.restore.conflict = un file diverso è già dove appartiene almeno uno di questi file, quindi il file già presente è stato conservato e la copia di backup non è stata ripristinata

journal.cli.backup.offload.restore.reason.restore.held = il ripristino dei file trasferiti è temporaneamente non disponibile

journal.cli.backup.offload.restore.reason.restore.not.published = non è stato possibile rimettere almeno un file nel tuo diario questa volta e non è stato messo niente al suo posto, quindi riprova più tardi

journal.cli.backup.offload.restore.reason.segment.identity = non è stato possibile verificare una delle registrazioni del tuo diario rispetto al suo registro di identità

journal.cli.backup.offload.restore.reason.segment.missing = la posizione del diario a cui appartengono questi file non esiste più su questo computer

journal.cli.backup.offload.restore.reason.stream.marker.failed = i file sono stati ripristinati, ma l'aggiornamento successivo dei registri del diario non è riuscito

journal.cli.backup.offload.restore.reason.timeout = è durato troppo e si è fermato

journal.cli.backup.offload.restore.reason.unknown.code = si è fermato senza un motivo ({ $code })

journal.cli.backup.offload.restore.reason.verification.failed = un file ripristinato non corrispondeva a ciò che avrebbe dovuto essere, quindi potrebbe essere danneggiato

journal.cli.backup.offload.restore.unfinished = il ripristino non si è concluso in modo pulito, quindi riprova più tardi.

journal.cli.backup.offload.status.heading = trasferimento del backup:

journal.cli.backup.offload.status.invalid = stato del trasferimento non valido.

journal.cli.backup.offload.status.overflow = overflow del totale dello stato dei contenuti multimediali

journal.cli.backup.offload.use.installed = esegui `{ $command_offload_run }` con il comando { $program } installato.

journal.cli.backup.output.json = non è stato possibile generare l'output { $json_format }.

journal.cli.backup.pause.already = il backup è già in pausa.

journal.cli.backup.pause.complete = backup in pausa. esegui `{ $command_resume }` per avviarlo di nuovo.

journal.cli.backup.pause.off = il backup è disattivo, quindi non c'è niente da mettere in pausa.

journal.cli.backup.payload.backend = Manca il backend.

journal.cli.backup.payload.credential = Manca la credenziale: { $key }.

journal.cli.backup.payload.credentials = Mancano le credenziali.

journal.cli.backup.payload.recovery.key = Manca { $key_field }.

journal.cli.backup.payload.repository = Manca il repository.

journal.cli.backup.payload.unsupported = Backend non supportato.

journal.cli.backup.prune.complete = Sfoltimento della conservazione completato.

journal.cli.backup.prune.detail.failure.rclone.unavailable = Sfoltimento non riuscito: il programma { $rclone_program } non è disponibile. { $detail_field }{ $detail }

journal.cli.backup.prune.detail.failure.restic.unavailable = Sfoltimento non riuscito: il programma di backup { $restic_program } non è disponibile. { $detail_field }{ $detail }

journal.cli.backup.prune.failure.access = Sfoltimento non riuscito: la destinazione ha rifiutato il permesso per questa scrittura.

journal.cli.backup.prune.failure.auth.failed = Sfoltimento non riuscito: la destinazione ha rifiutato le credenziali.

journal.cli.backup.prune.failure.binding.invalid = Sfoltimento non riuscito: le impostazioni di backup del tuo diario non sono più valide.

journal.cli.backup.prune.failure.binding.superseded = Sfoltimento non riuscito: le impostazioni di backup più recenti hanno sostituito queste.

journal.cli.backup.prune.failure.broker.error = Sfoltimento non riuscito: per il servizio di backup che { $company } gestisce, non è stato possibile completare la richiesta.

journal.cli.backup.prune.failure.broker.unreachable = Sfoltimento non riuscito: non è stato possibile connettersi al servizio di backup che { $company } gestisce.

journal.cli.backup.prune.failure.bucket.not.found = Sfoltimento non riuscito: le tue credenziali non possono aprire il bucket che il repository nomina.

journal.cli.backup.prune.failure.failed = Sfoltimento non riuscito: non è stato dato alcun motivo.

journal.cli.backup.prune.failure.hosted.entitlement.inactive = Sfoltimento non riuscito: { $company } non gestisce il backup per te.

journal.cli.backup.prune.failure.journal.path.unresolved = Sfoltimento non riuscito: non è stato possibile trovare il tuo diario.

journal.cli.backup.prune.failure.locked = Sfoltimento non riuscito: il backup alla destinazione è bloccato, di solito da un'esecuzione ancora in corso o da una che è stata interrotta.

journal.cli.backup.prune.failure.rclone.unavailable = Sfoltimento non riuscito: il programma { $rclone_program } non è disponibile.

journal.cli.backup.prune.failure.repo.exists = Sfoltimento non riuscito: c'è già un backup lì.

journal.cli.backup.prune.failure.repo.missing = Sfoltimento non riuscito: non c'è ancora un backup alla destinazione.

journal.cli.backup.prune.failure.repository.unsupported = Sfoltimento non riuscito: il repository nelle tue impostazioni di backup non inizia con { $s3_backend } o { $b2_backend } per corrispondere al suo backend.

journal.cli.backup.prune.failure.restic.unavailable = Sfoltimento non riuscito: il programma di backup { $restic_program } non è disponibile.

journal.cli.backup.prune.failure.timeout = Sfoltimento non riuscito: è durato troppo e si è fermato.

journal.cli.backup.prune.failure.unknown.code = Sfoltimento non riuscito: si è fermato senza un motivo ({ $code }).

journal.cli.backup.prune.failure.unreachable = Sfoltimento non riuscito: non è stato possibile connettersi alla destinazione.

journal.cli.backup.prune.skipped = Sfoltimento saltato (non attivo o non configurato).

journal.cli.backup.recovery.key.missing = Nessuna chiave di ripristino è impostata.

journal.cli.backup.restore.already.set.up = non è stato possibile avviare il ripristino: il tuo diario è già configurato, quindi non si può sovrascrivere con un ripristino. per usare un backup, avvia un nuovo diario e scegli il ripristino durante la configurazione iniziale.

journal.cli.backup.restore.complete = ripristino completato: { $counters }, { $integrity_field }{ $integrity }, { $resume_field }{ $resumable }.

journal.cli.backup.restore.degraded.integrity.failed = ripristino finito con un problema: { $counters }; il backup non ha superato il controllo di integrità e potrebbe essere danneggiato, quindi il tuo diario non è impostato sul backup ({ $reason_field }{ $code }).

journal.cli.backup.restore.degraded.integrity.unverified = ripristino finito con un problema: { $counters }; non è stato possibile eseguire il controllo di integrità (il backup era occupato o il tempo è scaduto), quindi il tuo diario non è impostato sul backup ({ $reason_field }{ $code }).

journal.cli.backup.restore.degraded.other = ripristino finito con un problema: { $counters }; il ripristino è stato completato con prova degradata ({ $reason_field }{ $code }).

journal.cli.backup.restore.degraded.restore.summary.missing = ripristino finito con un problema: { $counters }; il riepilogo del ripristino era mancante o malformato ({ $reason_field }{ $code }).

journal.cli.backup.restore.failed.counters = ripristino non riuscito: { $counters }.

journal.cli.backup.restore.failure.body.rebuild.failed = ripristino non riuscito: non è stato possibile ricostruire il tuo diario dai file ripristinati; { $counters }.

journal.cli.backup.restore.failure.destination.admission.failed = ripristino non riuscito: non è stato possibile connettersi alla destinazione del backup; { $counters }.

journal.cli.backup.restore.failure.destination.invalid = ripristino non riuscito: la destinazione del backup non è valida; { $counters }.

journal.cli.backup.restore.failure.destination.publish.failed = ripristino non riuscito: non è stato possibile salvare il diario ripristinato nella sua destinazione; { $counters }.

journal.cli.backup.restore.failure.full.scan.failed = ripristino non riuscito: una scansione completa del diario ripristinato non è riuscita; { $counters }.

journal.cli.backup.restore.failure.insufficient.free.space = ripristino non riuscito: non c'è abbastanza spazio libero per questo ripristino, quindi non è cambiato niente; { $counters }.

journal.cli.backup.restore.failure.integrity.check.io.failed = ripristino non riuscito: non è stato possibile controllare i file ripristinati; { $counters }.

journal.cli.backup.restore.failure.integrity.failed = ripristino non riuscito: il backup non ha superato il controllo di integrità e potrebbe essere danneggiato, quindi il tuo diario non è impostato sul backup; { $counters }.

journal.cli.backup.restore.failure.integrity.unverified = ripristino non riuscito: non è stato possibile eseguire il controllo di integrità (il backup era occupato o il tempo è scaduto), quindi il tuo diario non è impostato sul backup; { $counters }.

journal.cli.backup.restore.failure.invalid.key = ripristino non riuscito: la chiave di ripristino inserita non corrisponde a questo backup; { $counters }.

journal.cli.backup.restore.failure.journal.already.set.up = ripristino non riuscito: il tuo diario è già configurato, quindi non si può sovrascrivere con un ripristino. per usare un backup, avvia un nuovo diario e scegli il ripristino durante la configurazione iniziale; { $counters }.

journal.cli.backup.restore.failure.journal.snapshot.not.found = ripristino non riuscito: non è stato trovato nessun backup del tuo diario; { $counters }.

journal.cli.backup.restore.failure.recovery.confirmation.publish.failed = ripristino non riuscito: non è stato possibile salvare la conferma di ripristino; { $counters }.

journal.cli.backup.restore.failure.recovery.key.publish.failed = ripristino non riuscito: non è stato possibile salvare la chiave di ripristino; { $counters }.

journal.cli.backup.restore.failure.restic.unavailable = ripristino non riuscito: il programma di backup { $restic_program } non è disponibile; { $counters }.

journal.cli.backup.restore.failure.restore.failed = ripristino non riuscito: il ripristino non è stato completato; { $counters }.

journal.cli.backup.restore.failure.restore.io.failed = ripristino non riuscito: non è stato possibile scrivere i file durante il ripristino; { $counters }.

journal.cli.backup.restore.failure.restore.record.failed = ripristino non riuscito: non è stato possibile registrare il risultato del ripristino; { $counters }.

journal.cli.backup.restore.failure.restore.summary.missing = ripristino non riuscito: il ripristino è finito ma manca il suo riepilogo; { $counters }.

journal.cli.backup.restore.failure.snapshot.catalog.invalid = ripristino non riuscito: l'elenco dei backup non è valido; { $counters }.

journal.cli.backup.restore.failure.snapshot.list.failed = ripristino non riuscito: non è stato possibile recuperare l'elenco dei backup; { $counters }.

journal.cli.backup.restore.failure.snapshot.list.io.failed = ripristino non riuscito: non è stato possibile leggere l'elenco dei backup; { $counters }.

journal.cli.backup.restore.failure.snapshot.selection.ambiguous = ripristino non riuscito: più di un backup corrispondeva e non è stato possibile sceglierne uno automaticamente; { $counters }.

journal.cli.backup.restore.failure.unknown.code = ripristino non riuscito: si è fermato senza un motivo ({ $code }); { $counters }.

journal.cli.backup.restore.recording.failure.body.rebuild.failed = Anche registrare il risultato non è riuscito: non è stato possibile ricostruire il tuo diario dai file ripristinati.

journal.cli.backup.restore.recording.failure.destination.admission.failed = Anche registrare il risultato non è riuscito: non è stato possibile connettersi alla destinazione del backup.

journal.cli.backup.restore.recording.failure.destination.invalid = Anche registrare il risultato non è riuscito: la destinazione del backup non è valida.

journal.cli.backup.restore.recording.failure.destination.publish.failed = Anche registrare il risultato non è riuscito: non è stato possibile salvare il diario ripristinato nella sua destinazione.

journal.cli.backup.restore.recording.failure.full.scan.failed = Anche registrare il risultato non è riuscito: una scansione completa del diario ripristinato non è riuscita.

journal.cli.backup.restore.recording.failure.insufficient.free.space = Anche registrare il risultato non è riuscito: non c'è abbastanza spazio libero per questo ripristino, quindi non è cambiato niente.

journal.cli.backup.restore.recording.failure.integrity.check.io.failed = Anche registrare il risultato non è riuscito: non è stato possibile controllare i file ripristinati.

journal.cli.backup.restore.recording.failure.integrity.failed = Anche registrare il risultato non è riuscito: il backup non ha superato il controllo di integrità e potrebbe essere danneggiato, quindi il tuo diario non è impostato sul backup.

journal.cli.backup.restore.recording.failure.integrity.unverified = Anche registrare il risultato non è riuscito: non è stato possibile eseguire il controllo di integrità (il backup era occupato o il tempo è scaduto), quindi il tuo diario non è impostato sul backup.

journal.cli.backup.restore.recording.failure.invalid.key = Anche registrare il risultato non è riuscito: la chiave di ripristino inserita non corrisponde a questo backup.

journal.cli.backup.restore.recording.failure.journal.already.set.up = Anche registrare il risultato non è riuscito: il tuo diario è già configurato, quindi non si può sovrascrivere con un ripristino. per usare un backup, avvia un nuovo diario e scegli il ripristino durante la configurazione iniziale.

journal.cli.backup.restore.recording.failure.journal.snapshot.not.found = Anche registrare il risultato non è riuscito: non è stato trovato nessun backup del tuo diario.

journal.cli.backup.restore.recording.failure.recovery.confirmation.publish.failed = Anche registrare il risultato non è riuscito: non è stato possibile salvare la conferma di ripristino.

journal.cli.backup.restore.recording.failure.recovery.key.publish.failed = Anche registrare il risultato non è riuscito: non è stato possibile salvare la chiave di ripristino.

journal.cli.backup.restore.recording.failure.restic.unavailable = Anche registrare il risultato non è riuscito: il programma di backup { $restic_program } non è disponibile.

journal.cli.backup.restore.recording.failure.restore.failed = Anche registrare il risultato non è riuscito: il ripristino non è stato completato.

journal.cli.backup.restore.recording.failure.restore.io.failed = Anche registrare il risultato non è riuscito: non è stato possibile scrivere i file durante il ripristino.

journal.cli.backup.restore.recording.failure.restore.record.failed = Anche registrare il risultato non è riuscito: non è stato possibile registrare il risultato del ripristino.

journal.cli.backup.restore.recording.failure.restore.summary.missing = Anche registrare il risultato non è riuscito: il ripristino è finito ma manca il suo riepilogo.

journal.cli.backup.restore.recording.failure.snapshot.catalog.invalid = Anche registrare il risultato non è riuscito: l'elenco dei backup non è valido.

journal.cli.backup.restore.recording.failure.snapshot.list.failed = Anche registrare il risultato non è riuscito: non è stato possibile recuperare l'elenco dei backup.

journal.cli.backup.restore.recording.failure.snapshot.list.io.failed = Anche registrare il risultato non è riuscito: non è stato possibile leggere l'elenco dei backup.

journal.cli.backup.restore.recording.failure.snapshot.selection.ambiguous = Anche registrare il risultato non è riuscito: più di un backup corrispondeva e non è stato possibile sceglierne uno automaticamente.

journal.cli.backup.restore.recording.failure.unknown.code = Anche registrare il risultato non è riuscito: si è fermato senza un motivo ({ $code }).

journal.cli.backup.restore.unrecorded = ripristino completato, ma registrare il risultato non è riuscito: { $counters } ({ $reason_field }{ $restore_record }).

journal.cli.backup.resume.complete = backup ripreso. il prossimo backup viene eseguito secondo la sua pianificazione abituale.

journal.cli.backup.resume.not.paused = il backup non è in pausa.

journal.cli.backup.rotation.failed = Rotazione non riuscita: non riuscita.

journal.cli.backup.rotation.failure.access = Rotazione non riuscita: la destinazione ha rifiutato il permesso per questa scrittura.

journal.cli.backup.rotation.failure.auth.failed = Rotazione non riuscita: la destinazione ha rifiutato le credenziali.

journal.cli.backup.rotation.failure.binding.invalid = Rotazione non riuscita: le impostazioni di backup del tuo diario non sono più valide.

journal.cli.backup.rotation.failure.binding.superseded = Rotazione non riuscita: le impostazioni di backup più recenti hanno sostituito queste.

journal.cli.backup.rotation.failure.broker.error = Rotazione non riuscita: per il servizio di backup che { $company } gestisce, non è stato possibile completare la richiesta.

journal.cli.backup.rotation.failure.broker.unreachable = Rotazione non riuscita: non è stato possibile connettersi al servizio di backup che { $company } gestisce.

journal.cli.backup.rotation.failure.bucket.not.found = Rotazione non riuscita: le tue credenziali non possono aprire il bucket che il repository nomina.

journal.cli.backup.rotation.failure.failed = Rotazione non riuscita: non è stato dato alcun motivo.

journal.cli.backup.rotation.failure.hosted.entitlement.inactive = Rotazione non riuscita: { $company } non gestisce il backup per te.

journal.cli.backup.rotation.failure.journal.path.unresolved = Rotazione non riuscita: non è stato possibile trovare il tuo diario.

journal.cli.backup.rotation.failure.locked = Rotazione non riuscita: il backup alla destinazione è bloccato, di solito da un'esecuzione ancora in corso o da una che è stata interrotta.

journal.cli.backup.rotation.failure.rclone.unavailable = Rotazione non riuscita: il programma { $rclone_program } non è disponibile.

journal.cli.backup.rotation.failure.repo.exists = Rotazione non riuscita: c'è già un backup lì.

journal.cli.backup.rotation.failure.repo.missing = Rotazione non riuscita: non c'è ancora un backup alla destinazione.

journal.cli.backup.rotation.failure.repository.unsupported = Rotazione non riuscita: il repository nelle tue impostazioni di backup non inizia con { $s3_backend } o { $b2_backend } per corrispondere al suo backend.

journal.cli.backup.rotation.failure.restic.unavailable = Rotazione non riuscita: il programma di backup { $restic_program } non è disponibile.

journal.cli.backup.rotation.failure.timeout = Rotazione non riuscita: è durata troppo e si è fermata.

journal.cli.backup.rotation.failure.unknown.code = Rotazione non riuscita: si è fermata senza un motivo ({ $code }).

journal.cli.backup.rotation.failure.unreachable = Rotazione non riuscita: non è stato possibile connettersi alla destinazione.

journal.cli.backup.rotation.key = Nuova chiave di ripristino (annotala):{ $line_break }{ $key_grid }

journal.cli.backup.rotation.skipped = Rotazione della chiave di ripristino saltata (backup non configurato).

journal.cli.backup.run.complete = Backup completato (istantanea { $snapshot }).

journal.cli.backup.run.failure.access = Backup non riuscito: la destinazione ha rifiutato il permesso per questa scrittura.

journal.cli.backup.run.failure.auth.failed = Backup non riuscito: la destinazione ha rifiutato le credenziali.

journal.cli.backup.run.failure.binding.invalid = Backup non riuscito: le impostazioni di backup del tuo diario non sono più valide.

journal.cli.backup.run.failure.binding.superseded = Backup non riuscito: le impostazioni di backup più recenti hanno sostituito queste.

journal.cli.backup.run.failure.broker.error = Backup non riuscito: per il servizio di backup che { $company } gestisce, non è stato possibile completare la richiesta.

journal.cli.backup.run.failure.broker.unreachable = Backup non riuscito: non è stato possibile connettersi al servizio di backup che { $company } gestisce.

journal.cli.backup.run.failure.bucket.not.found = Backup non riuscito: le tue credenziali non possono aprire il bucket che il repository nomina.

journal.cli.backup.run.failure.failed = Backup non riuscito: non è stato dato alcun motivo.

journal.cli.backup.run.failure.hosted.entitlement.inactive = Backup non riuscito: { $company } non gestisce il backup per te.

journal.cli.backup.run.failure.journal.path.unresolved = Backup non riuscito: non è stato possibile trovare il tuo diario.

journal.cli.backup.run.failure.locked = Backup non riuscito: il backup alla destinazione è bloccato, di solito da un'esecuzione ancora in corso o da una che è stata interrotta.

journal.cli.backup.run.failure.rclone.unavailable = Backup non riuscito: il programma { $rclone_program } non è disponibile.

journal.cli.backup.run.failure.repo.exists = Backup non riuscito: c'è già un backup lì.

journal.cli.backup.run.failure.repo.missing = Backup non riuscito: non c'è ancora un backup alla destinazione.

journal.cli.backup.run.failure.repository.unsupported = Backup non riuscito: il repository nelle tue impostazioni di backup non inizia con { $s3_backend } o { $b2_backend } per corrispondere al suo backend.

journal.cli.backup.run.failure.restic.unavailable = Backup non riuscito: il programma di backup { $restic_program } non è disponibile.

journal.cli.backup.run.failure.timeout = Backup non riuscito: è durato troppo e si è fermato.

journal.cli.backup.run.failure.unknown.code = Backup non riuscito: si è fermato senza un motivo ({ $code }).

journal.cli.backup.run.failure.unreachable = Backup non riuscito: non è stato possibile connettersi alla destinazione.

journal.cli.backup.run.partial.snapshot.count = { $quantity ->
    [one] Backup parziale (istantanea { $snapshot }). non è stato possibile leggere 1 file, quindi non è incluso. Esegui di nuovo `{ $command }` una volta risolto.
    [many] Backup parziale (istantanea { $snapshot }). non è stato possibile leggere { $count } di file, quindi non sono inclusi. Esegui di nuovo `{ $command }` una volta risolto.
   *[other] Backup parziale (istantanea { $snapshot }). non è stato possibile leggere { $count } file, quindi non sono inclusi. Esegui di nuovo `{ $command }` una volta risolto.
}

journal.cli.backup.run.partial.snapshot.count.reason = { $quantity ->
    [one] Backup parziale (istantanea { $snapshot }). non è stato possibile leggere 1 file, quindi non è incluso. Il primo: { $reason }. Esegui di nuovo `{ $command }` una volta risolto.
    [many] Backup parziale (istantanea { $snapshot }). non è stato possibile leggere { $count } di file, quindi non sono inclusi. Il primo: { $reason }. Esegui di nuovo `{ $command }` una volta risolto.
   *[other] Backup parziale (istantanea { $snapshot }). non è stato possibile leggere { $count } file, quindi non sono inclusi. Il primo: { $reason }. Esegui di nuovo `{ $command }` una volta risolto.
}

journal.cli.backup.run.partial.snapshot.unmeasured = Backup parziale (istantanea { $snapshot }). non è stato possibile leggere alcuni file, quindi non sono inclusi. Esegui di nuovo `{ $command_run }` una volta risolto.

journal.cli.backup.run.partial.unconfirmed.count = { $quantity ->
    [one] Backup parziale. non è stato possibile leggere 1 file, quindi non è incluso, e non è stato possibile confermare l'istantanea. Esegui di nuovo `{ $command }` una volta risolto.
    [many] Backup parziale. non è stato possibile leggere { $count } di file, quindi non sono inclusi, e non è stato possibile confermare l'istantanea. Esegui di nuovo `{ $command }` una volta risolto.
   *[other] Backup parziale. non è stato possibile leggere { $count } file, quindi non sono inclusi, e non è stato possibile confermare l'istantanea. Esegui di nuovo `{ $command }` una volta risolto.
}

journal.cli.backup.run.partial.unconfirmed.count.reason = { $quantity ->
    [one] Backup parziale. non è stato possibile leggere 1 file, quindi non è incluso, e non è stato possibile confermare l'istantanea. Il primo: { $reason }. Esegui di nuovo `{ $command }` una volta risolto.
    [many] Backup parziale. non è stato possibile leggere { $count } di file, quindi non sono inclusi, e non è stato possibile confermare l'istantanea. Il primo: { $reason }. Esegui di nuovo `{ $command }` una volta risolto.
   *[other] Backup parziale. non è stato possibile leggere { $count } file, quindi non sono inclusi, e non è stato possibile confermare l'istantanea. Il primo: { $reason }. Esegui di nuovo `{ $command }` una volta risolto.
}

journal.cli.backup.run.partial.unconfirmed.unmeasured = Backup parziale. non è stato possibile leggere alcuni file, quindi non sono inclusi, e non è stato possibile confermare l'istantanea. Esegui di nuovo `{ $command_run }` una volta risolto.

journal.cli.backup.run.skipped = Backup saltato (non attivo o non configurato).

journal.cli.backup.teardown.backup.only.held = il tuo backup tiene l'unica copia di alcuni dei tuoi contenuti multimediali, quindi non è stata fatta nessuna modifica al backup. ripristina prima tutto dalla pagina del backup, poi riprova.

journal.cli.backup.teardown.confirm = Rifiuto di smantellare il backup senza { $yes_flag }. Questo dimentica tutte le istantanee.

journal.cli.backup.teardown.not.started = non è stato possibile avviare la disattivazione e l'eliminazione del tuo backup. non è stata fatta nessuna modifica al backup.

journal.cli.backup.teardown.partial = lo smantellamento del backup non è finito. conserva i file di ripristino e rivedi gli effetti registrati prima di riprovare.

journal.cli.backup.teardown.prelaunch.held = disattivare ed eliminare il tuo backup non è disponibile qui. non è stata fatta nessuna modifica al backup.

journal.cli.backup.usage.destination = { $command }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.backup.usage.missing.destination = { $command }: errore: i seguenti argomenti sono obbligatori: { $required }

journal.cli.backup.usage.missing.offload = { $command }: errore: i seguenti argomenti sono obbligatori: { $required }

journal.cli.backup.usage.missing.recovery.key = { $command }: errore: i seguenti argomenti sono obbligatori: { $required }

journal.cli.backup.usage.missing.root = { $command }: errore: i seguenti argomenti sono obbligatori: { $required }

journal.cli.backup.usage.offload = { $command }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.backup.usage.recovery.key = { $command }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.backup.usage.root = { $command }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.client.date.short = { $weekday } { $month } { $day }

journal.cli.client.date.short.year = { $weekday } { $month } { $day } '{ $year }

journal.cli.client.erase.backups.conflict = Errore: scegli { $from_backups } o { $skip_backups }, non entrambi.

journal.cli.client.erase.backups.missing = Errore: il backup non è configurato, quindi non ci sono backup da cui rimuoverlo.

journal.cli.client.erase.backups.next = Viene rimosso dai tuoi backup come passo successivo.

journal.cli.client.erase.backups.resume = Viene rimosso dai tuoi backup quando riprendi il backup.

journal.cli.client.erase.backups.unavailable = Errore: questo diario non può ancora rimuovere cose dai tuoi backup.

journal.cli.client.erase.backups.unchecked = Errore: non è stato possibile controllare i tuoi backup proprio ora. Riprova, oppure passa { $skip_backups } per cancellare senza rimuoverlo dai tuoi backup.

journal.cli.client.error.detail = Errore: { $detail }

journal.cli.client.error.malformed = non è stato possibile leggere la risposta del diario.

journal.cli.client.error.timeout = Il diario non ha risposto in tempo.

journal.cli.client.error.unreachable = non è stato possibile connettersi al tuo diario tramite { $http }.

journal.cli.client.error.unreadable = Il diario ha restituito un errore illeggibile.

journal.cli.client.lock.acquire = non è stato possibile acquisire il blocco sidecar: { $error }

journal.cli.client.lock.open = non è possibile aprire il file di blocco: { $error }

journal.cli.client.missing.argument = Errore: argomento mancante { $argument }

journal.cli.client.network.cli.pair.join.hint = collega questo dispositivo con:

journal.cli.client.network.cli.pair.local.network.closed = non è possibile avviare l'accoppiamento. il tuo diario è chiuso ai dispositivi sulla tua rete. esegui `{ $call_open }` per accoppiare un altro dispositivo sulla tua rete, oppure attiva la tua rete privata per accoppiare tramite il relay.

journal.cli.client.network.cli.pair.no.lan.address = non è possibile avviare l'accoppiamento. il tuo diario non è ancora raggiungibile su un indirizzo di rete. attiva la tua rete privata per accoppiare da qualsiasi luogo, oppure connetti questo dispositivo alla tua rete di casa.

journal.cli.client.network.clients.empty = Nessun client autorizzato.

journal.cli.client.network.clients.seen = { $fingerprint }  { $label }  visto l'ultima volta { $when }

journal.cli.client.network.device.fingerprint.absent = Nessun dispositivo accoppiato con impronta { $fingerprint }

journal.cli.client.network.device.label.absent = Nessun dispositivo accoppiato con etichetta { $label }

journal.cli.client.network.device.label.cleared = Etichetta del dispositivo impostata su: (vuota)

journal.cli.client.network.device.label.set = Etichetta del dispositivo impostata su: { $label }

journal.cli.client.network.devices.empty = Ancora nessun dispositivo collegato.

journal.cli.client.network.devices.seen = - { $label } · aggiunto { $added } · visto l'ultima volta { $seen } [{ $fingerprint }]

journal.cli.client.network.devices.total = Dispositivi accoppiati: { $total }

journal.cli.client.network.journal.id = id del diario:     { $id }

journal.cli.client.network.label.options = Errore: specifica { $label } oppure { $clear }

journal.cli.client.network.linked.systems.heading = Sistemi collegati:

journal.cli.client.network.listen.state = { $protocol } stato: (interroga { $convey } { $path } per lo stato in tempo reale)

journal.cli.client.network.local.network.open = il tuo diario è aperto ai dispositivi sulla tua rete. i dispositivi sulla stessa rete possono accoppiarsi direttamente al tuo diario.

journal.cli.client.network.local.network.this.pc = il tuo diario è chiuso ai dispositivi sulla tua rete. l'app di { $brand } su questo computer si accoppia ancora, come i dispositivi tramite il relay, una volta che la tua rete privata è attiva.

journal.cli.client.network.local.network.windows.asks = windows potrebbe chiedere se journal può usare la tua rete. con un account standard, un amministratore deve consentirlo. se windows non ha chiesto e da un dispositivo non è ancora stato possibile connettersi al tuo diario, una scelta precedente in windows potrebbe bloccare journal. un amministratore può consentirlo in { $security }, sotto { $firewall }.

journal.cli.client.network.mark.none = l'identità del tuo diario viene creata quando la sua marca viene fissata durante la configurazione iniziale.

journal.cli.client.network.mark.unavailable = marca del tuo diario: non disponibile in questo momento

journal.cli.client.network.observer.pause = { $verb } non è ancora disponibile.

journal.cli.client.network.operation = operazione: { $phase }

journal.cli.client.network.option.float = Errore: l'opzione richiede un valore in virgola mobile.

journal.cli.client.network.pair.complete = Richiesta di accoppiamento completata; il dispositivo dovrebbe comparire in `{ $command }`.

journal.cli.client.network.pair.device = Dispositivo: { $device }

journal.cli.client.network.pair.expired = Tempo scaduto. Codice di accoppiamento scaduto.

journal.cli.client.network.pair.fingerprint = { $ca } impronta: { $algorithm }{ $fingerprint }

journal.cli.client.network.pair.link = { $token }: { $link }

journal.cli.client.network.pair.paired = Accoppiato: { $label }

journal.cli.client.network.pair.result.date = { $indent }accoppiato il:{ $indent } { $date }

journal.cli.client.network.pair.result.fingerprint = { $indent }impronta: { $fingerprint }

journal.cli.client.network.pair.wait = In attesa del sistema collegato…

journal.cli.client.network.portal.approve = continua per approvare → { $url }

journal.cli.client.network.posture.direct = assetto: diretto

journal.cli.client.network.posture.private = assetto: rete privata

journal.cli.client.network.private.enrolled = iscrizione: sì

journal.cli.client.network.private.link.needs.subscription = la tua rete privata non è ancora attiva. il tuo consenso è salvato. finisci di attivarla nel portale dei servizi, poi attivala di nuovo qui:

journal.cli.client.network.private.link.setting.up = configurazione della tua rete privata in corso...

journal.cli.client.network.private.link.subscribing = la tua rete privata non è ancora attiva. il tuo consenso è salvato. finisci di attivarla nel portale dei servizi entro un'ora, e il tuo diario la rileva da solo:

journal.cli.client.network.private.state.enabled = stato: attivo

journal.cli.client.network.private.state.inconsistent = stato: da riparare

journal.cli.client.network.private.state.not.enabled = stato: non attivo

journal.cli.client.network.private.state.other = stato: { $state }

journal.cli.client.network.private.state.unknown = stato: sconosciuto

journal.cli.client.network.private.unenrolled = iscrizione: no

journal.cli.client.network.reach.direct = Reach assetto: diretto

journal.cli.client.network.reach.private = Reach assetto: rete privata

journal.cli.client.network.relay.url = url del relay: { $url }

journal.cli.client.network.role.invalid = ruolo non valido; è atteso uno tra: { $phone }, { $observer }

journal.cli.client.network.status.enrolled = Iscrizione:      sì

journal.cli.client.network.status.network.enabled = Rete privata: attiva

journal.cli.client.network.status.network.inconsistent = Rete privata: da riparare

journal.cli.client.network.status.network.not.enabled = Rete privata: non attiva

journal.cli.client.network.status.network.other = Rete privata: { $state }

journal.cli.client.network.status.network.unknown = Rete privata: sconosciuta

journal.cli.client.network.status.relay = URL del relay:     { $url }

journal.cli.client.network.status.relay.absent = URL del relay:     None

journal.cli.client.network.status.unenrolled = Iscrizione:      no

journal.cli.client.network.time.day = { $quantity ->
    [one] { $quantity } giorno fa
    [many] { $quantity } di giorni fa
   *[other] { $quantity } giorni fa
}

journal.cli.client.network.time.hour = { $quantity ->
    [one] { $quantity } ora fa
    [many] { $quantity } di ore fa
   *[other] { $quantity } ore fa
}

journal.cli.client.network.time.minute = { $quantity ->
    [one] { $quantity } minuto fa
    [many] { $quantity } di minuti fa
   *[other] { $quantity } minuti fa
}

journal.cli.client.network.time.month = { $quantity ->
    [one] { $quantity } mese fa
    [many] { $quantity } di mesi fa
   *[other] { $quantity } mesi fa
}

journal.cli.client.network.time.second = { $quantity ->
    [one] { $quantity } secondo fa
    [many] { $quantity } di secondi fa
   *[other] { $quantity } secondi fa
}

journal.cli.client.network.time.week = { $quantity ->
    [one] { $quantity } settimana fa
    [many] { $quantity } di settimane fa
   *[other] { $quantity } settimane fa
}

journal.cli.client.network.timeout.integer = Errore: l'opzione { $flag } richiede un intero.

journal.cli.client.network.unpaired = Disaccoppiato.

journal.cli.client.option.required = Errore: l'opzione { $option } è obbligatoria.

journal.cli.client.option.unknown = Errore: opzione sconosciuta { $option }.

journal.cli.client.option.value = Errore: l'opzione { $option } richiede un argomento.

journal.cli.client.port.invalid = { $variable } deve essere un intero da 1 a 65535.

journal.cli.client.stdin.read = lettura nativa di { $brand } { $input } non riuscita: { $error }

journal.cli.client.transport.timeout = { $method } { $path } ha superato il timeout locale di { $service } (connect={ $connect }s, read={ $read }s, total={ $total }s)

journal.cli.client.transport.timeout.open = { $method } { $path } ha superato il timeout locale di { $service } (connect={ $connect }s, read=none, total=none)

journal.cli.client.trash.moved = Spostato '{ $name }' nel cestino.

journal.cli.client.trash.moved.day = Spostato '{ $name }' nel cestino. Viene cancellato { $day }, a meno che non venga ripristinato.

journal.cli.client.trash.moved.failed = Spostato '{ $name }' nel cestino. Non è stato possibile cancellarlo ora.

journal.cli.client.trash.moved.failed.day = Spostato '{ $name }' nel cestino. Non è stato possibile cancellarlo ora. Viene cancellato { $day }, a meno che non venga ripristinato.

journal.cli.importer.arg.archive.conflict.commands = argomenti non riconosciuti: comandi dell'archivio audio in conflitto

journal.cli.importer.arg.archive.conflict.flags = argomenti non riconosciuti: flag dell'archivio audio in conflitto

journal.cli.importer.arg.archive.identity.scope = --archive-identity è valido solo con --audio-archive

journal.cli.importer.arg.archive.identity.sha256 = --archive-identity deve essere un'identità SHA-256

journal.cli.importer.arg.error = solstone journal importer: errore: { $detail }

journal.cli.importer.arg.expected.one = argomento { $argument }: atteso un argomento

journal.cli.importer.arg.facet.scope = --facet è valido solo quando si importa un solo file audio o di testo oppure un'importazione del corpo

journal.cli.importer.arg.invalid.int = argomento { $flag }: valore int non valido: '{ $value }'

journal.cli.importer.arg.media.required = i seguenti argomenti sono obbligatori: media

journal.cli.importer.arg.offset.limit.scope = argomenti non riconosciuti: --offset/--limit sono validi solo con --audio-archive-members

journal.cli.importer.arg.resume.scope = argomenti non riconosciuti: --resume è valido solo con --audio-archive

journal.cli.importer.arg.source.zone.scope = argomenti non riconosciuti: --source-zone è valido solo con --audio-archive

journal.cli.importer.arg.unrecognized = argomenti non riconosciuti: { $arguments }

journal.cli.importer.arg.usage = uso: solstone journal importer [-h] [opzioni] media [timestamp]

journal.cli.importer.audio.archive.admitted.line1 = archivio audio { $run_id }: { $members }, { $source_bytes }, { $days }, { $tiles }

journal.cli.importer.audio.archive.changed = l'archivio audio è cambiato dopo l'anteprima; mostra di nuovo l'anteprima

journal.cli.importer.audio.archive.complete.line1 = archivio audio { $operation }: { $run_id }

journal.cli.importer.audio.archive.complete.line2 = salvataggio di { $parts } in { $tiles }; { $pending } in attesa di elaborazione, { $processed } con elaborazione conclusa, { $unavailable } nello stato non disponibile, { $rejected } nello stato di rifiuto

journal.cli.importer.audio.archive.dry.run.resume = usa --audio-archive-status per ispezionare un'esecuzione ammessa

journal.cli.importer.audio.archive.failed = archivio audio non riuscito: { $error }

journal.cli.importer.audio.archive.folder.needs.zone = una cartella richiede --source-zone

journal.cli.importer.audio.archive.left.out.header = esclusi:

journal.cli.importer.audio.archive.left.out.inline = esclusi: { $counts }

journal.cli.importer.audio.archive.members.failed = non è stato possibile elencare i membri dell'archivio audio: { $error }

journal.cli.importer.audio.archive.members.summary = membri dell'archivio audio: run_id={ $run_id } offset={ $offset } count={ $count }

journal.cli.importer.audio.archive.preview.line1 = anteprima dell'archivio audio: { $members }, { $source_bytes }, { $days }, { $tiles }

journal.cli.importer.audio.archive.progress.saved = salvataggio { $saved } di { $total } ({ $tiles })

journal.cli.importer.audio.archive.span.line = intervallo: { $span }

journal.cli.importer.audio.archive.span.none = nessuno

journal.cli.importer.audio.archive.span.range = da { $first } a { $last }

journal.cli.importer.audio.archive.status.failed = non è stato possibile leggere lo stato dell'archivio audio: { $error }

journal.cli.importer.audio.archive.status.summary = stato dell'archivio audio: run_id={ $run_id } total_parts={ $total_parts } saved_parts={ $saved_parts } saved_tiles={ $saved_tiles } pending_processing={ $pending } processed_parts={ $processed } unavailable_parts={ $unavailable } rejected_parts={ $rejected } save_complete={ $save_complete }

journal.cli.importer.audio.archive.zone.changed = il fuso orario è cambiato dopo l'anteprima; mostra di nuovo l'anteprima

journal.cli.importer.audio.complete = Importazione audio generica completata: { $outcome }

journal.cli.importer.audio.dropped.notice = non è stato possibile importare parte di questa registrazione: { $ranges } nella registrazione. le altre parti sono nel tuo diario, e importare di nuovo il file le aggiungerebbe una seconda volta.

journal.cli.importer.audio.dropped.range = da { $start } a { $end }

journal.cli.importer.audio.failed = importazione audio non riuscita: { $error }

journal.cli.importer.audio.manifest.failed = l'importazione audio è stata salvata nel tuo diario, ma non è stato possibile segnare questo file come importato, quindi importarlo di nuovo lo aggiungerebbe una seconda volta: { $error }

journal.cli.importer.audio.processing.failed = elaborazione dell'importazione audio non riuscita: { $segments }

journal.cli.importer.audio.processing.stalled = l'importazione audio era ancora in elaborazione quando l'importatore ha smesso di aspettare: { $segments }

journal.cli.importer.audio.runtime.failed = ambiente di esecuzione dell'importazione audio non riuscito: { $error }

journal.cli.importer.backends.header = Backend sincronizzabili:

journal.cli.importer.body.apple.already.ran = questa importazione è già stata eseguita, quindi non è stato importato niente. avviane una nuova dalla scheda di Apple Health.

journal.cli.importer.body.apple.card.only = --import-id serve solo a salvare un'esportazione di Apple Health dalla sua scheda, quindi non è stato importato niente.

journal.cli.importer.body.apple.preview.intro = sembra un'esportazione di Apple Health. salvarla legge { $head }, e { $tail }.

journal.cli.importer.body.apple.preview.moves = { $quantity ->
    [one] dire di sì può spostare fino a 1 registro già elencato sotto un'altra facetta.
    [many] dire di sì può spostare fino a { $count } di registri già elencati sotto un'altra facetta.
   *[other] dire di sì può spostare fino a { $count } registri già elencati sotto un'altra facetta.
}

journal.cli.importer.body.apple.preview.span = va da { $first } a { $last }: { $workouts }, { $nights } messe insieme dalle fasi del sonno, { $mindful }, { $readings }, { $daily } e { $logged }.

journal.cli.importer.body.apple.reads.daily = totali del giorno come passi e distanza

journal.cli.importer.body.apple.reads.logged = elementi annotati come cibo e sintomi

journal.cli.importer.body.apple.reads.mindful = tempo di consapevolezza

journal.cli.importer.body.apple.reads.readings = frequenza cardiaca e altre letture

journal.cli.importer.body.apple.reads.sleep = sonno

journal.cli.importer.body.apple.reads.workouts = allenamenti

journal.cli.importer.body.apple.unknown.import = --import-id non nomina nessuna importazione, quindi non è stato importato niente.

journal.cli.importer.body.apple.whole = un'importazione di Apple Health legge sempre l'intera esportazione, quindi non accetta --date-from, --date-to o --force. non è stato salvato niente; eseguila di nuovo senza di essi.

journal.cli.importer.body.apple.wrong.export = quell'importazione non tiene questa esportazione, quindi non è stato importato niente.

journal.cli.importer.body.days.hidden = { $quantity ->
    [one] 1 giorno di dati del corpo è stato nascosto nell'archivio compresso, quindi è nascosto anche in questo diario.
    [many] { $count } di giorni di dati del corpo sono stati nascosti nell'archivio compresso, quindi sono nascosti anche in questo diario.
   *[other] { $count } giorni di dati del corpo sono stati nascosti nell'archivio compresso, quindi sono nascosti anche in questo diario.
}

journal.cli.importer.body.facet.ask.existing = archivia i tuoi allenamenti, il sonno e gli altri dati del corpo sotto la facetta a cui hai dato il nome { $facet }?

journal.cli.importer.body.facet.ask.make.body = crea una facetta corpo per i tuoi allenamenti, il sonno e gli altri dati del corpo?

journal.cli.importer.body.facet.ask.unmute = la tua facetta corpo è silenziata. riattivala per i tuoi allenamenti, il sonno e gli altri dati del corpo?

journal.cli.importer.body.facet.moves.body = { $quantity ->
    [one] un sì può spostare fino a 1 registro già elencato nella pagina delle attività, da { $from }.
    [many] un sì può spostare fino a { $count } di registri già elencati nella pagina delle attività, da { $from }.
   *[other] un sì può spostare fino a { $count } registri già elencati nella pagina delle attività, da { $from }.
}

journal.cli.importer.body.facet.moves.strava = { $quantity ->
    [one] un sì sposta 1 allenamento già archiviato, da { $from }.
    [many] un sì sposta { $count } di allenamenti già archiviati, da { $from }.
   *[other] un sì sposta { $count } allenamenti già archiviati, da { $from }.
}

journal.cli.importer.body.facet.no.means.body = se dici no, i nuovi allenamenti e il sonno non sono elencati nella pagina delle attività, e gli agenti limitati ad alcune facette non li vedono. la ricerca e l'app del corpo li mostrano ancora.

journal.cli.importer.body.facet.no.means.strava = se dici no, i nuovi allenamenti non sono elencati nella pagina delle attività, non ne derivano persone o cose, e gli agenti limitati ad alcune facette non li vedono. la ricerca e l'app del corpo li mostrano ancora.

journal.cli.importer.body.facet.prompt.yn = { $question } [y/n]

journal.cli.importer.body.facet.save.failed = non è stato possibile salvare la tua risposta: { $error }

journal.cli.importer.body.oura.bad.date = { $flag } accetta una data come 2026-01-31, e { $text } non lo è. non è stato letto niente.

journal.cli.importer.body.oura.date.to.needs.from = --date-to ha bisogno di un --date-from che lo accompagni. non è stato letto niente.

journal.cli.importer.body.oura.range.future = quell'intervallo va oltre oggi, e Oura non ha ancora niente lì. non è stato letto niente.

journal.cli.importer.body.oura.window.conflict = usa --window-days o le date, non entrambi. non è stato letto niente.

journal.cli.importer.body.oura.window.zero = --window-days ha bisogno di almeno un giorno. non è stato letto niente.

journal.cli.importer.body.zones.hours = { $quantity ->
    [one] 1 ora di { $source } può comparire all'ora sbagliata: è stata salvata in un fuso orario diverso da quello che questo diario tiene per { $source }.
    [many] { $count } di ore di { $source } possono comparire all'ora sbagliata: sono state salvate in un fuso orario diverso da quello che questo diario tiene per { $source }.
   *[other] { $count } ore di { $source } possono comparire all'ora sbagliata: sono state salvate in un fuso orario diverso da quello che questo diario tiene per { $source }.
}

journal.cli.importer.body.zones.under.hour = meno di un'ora di { $source } può comparire all'ora sbagliata: è stata salvata in un fuso orario diverso da quello che questo diario tiene per { $source }.

journal.cli.importer.connect.available = Backend collegabili: oura, plaud

journal.cli.importer.connect.oura.both.required = Oura ha bisogno sia del client ID sia del client secret, quindi non è stato salvato niente.

journal.cli.importer.connect.oura.done = Oura è connesso, e la connessione è salvata nelle impostazioni di questo diario.

journal.cli.importer.connect.oura.scopes = permessi richiesti: { $scopes }

journal.cli.importer.connect.unknown.backend = Backend di connessione sconosciuto: { $backend }

journal.cli.importer.count.attendee = { $quantity ->
    [one] 1 persona partecipante distinta
    [many] { $count } di persone partecipanti distinte
   *[other] { $count } persone partecipanti distinte
}

journal.cli.importer.count.calendar.event = { $quantity ->
    [one] 1 evento del calendario
    [many] { $count } di eventi del calendario
   *[other] { $count } eventi del calendario
}

journal.cli.importer.count.conversation = { $quantity ->
    [one] 1 conversazione
    [many] { $count } di conversazioni
   *[other] { $count } conversazioni
}

journal.cli.importer.count.daily.total = { $quantity ->
    [one] 1 totale del giorno
    [many] { $count } di totali del giorno
   *[other] { $count } totali del giorno
}

journal.cli.importer.count.day = { $quantity ->
    [one] 1 giorno
    [many] { $count } di giorni
   *[other] { $count } giorni
}

journal.cli.importer.count.entity = { $quantity ->
    [one] 1 entità
    [many] { $count } di entità
   *[other] { $count } entità
}

journal.cli.importer.count.event = { $quantity ->
    [one] 1 evento
    [many] { $count } di eventi
   *[other] { $count } eventi
}

journal.cli.importer.count.facet = { $quantity ->
    [one] 1 facetta
    [many] { $count } di facette
   *[other] { $count } facette
}

journal.cli.importer.count.logged.entry = { $quantity ->
    [one] 1 elemento annotato
    [many] { $count } di elementi annotati
   *[other] { $count } elementi annotati
}

journal.cli.importer.count.member = { $quantity ->
    [one] 1 membro
    [many] { $count } di membri
   *[other] { $count } membri
}

journal.cli.importer.count.message = { $quantity ->
    [one] 1 messaggio
    [many] { $count } di messaggi
   *[other] { $count } messaggi
}

journal.cli.importer.count.mindful.session = { $quantity ->
    [one] 1 sessione di consapevolezza
    [many] { $count } di sessioni di consapevolezza
   *[other] { $count } sessioni di consapevolezza
}

journal.cli.importer.count.night = { $quantity ->
    [one] 1 notte
    [many] { $count } di notti
   *[other] { $count } notti
}

journal.cli.importer.count.part = { $quantity ->
    [one] 1 parte
    [many] { $count } di parti
   *[other] { $count } parti
}

journal.cli.importer.count.pdf.document = { $quantity ->
    [one] 1 documento PDF
    [many] { $count } di documenti PDF
   *[other] { $count } documenti PDF
}

journal.cli.importer.count.reading = { $quantity ->
    [one] 1 lettura
    [many] { $count } di letture
   *[other] { $count } letture
}

journal.cli.importer.count.recording = { $quantity ->
    [one] 1 registrazione
    [many] { $count } di registrazioni
   *[other] { $count } registrazioni
}

journal.cli.importer.count.segment = { $quantity ->
    [one] 1 segmento
    [many] { $count } di segmenti
   *[other] { $count } segmenti
}

journal.cli.importer.count.source.byte = { $quantity ->
    [one] 1 byte di origine
    [many] { $count } di byte di origine
   *[other] { $count } byte di origine
}

journal.cli.importer.count.tile = { $quantity ->
    [one] 1 riquadro
    [many] { $count } di riquadri
   *[other] { $count } riquadri
}

journal.cli.importer.count.total.page = { $quantity ->
    [one] 1 pagina totale
    [many] { $count } di pagine totali
   *[other] { $count } pagine totali
}

journal.cli.importer.count.warning = { $quantity ->
    [one] 1 avviso
    [many] { $count } di avvisi
   *[other] { $count } avvisi
}

journal.cli.importer.count.workout = { $quantity ->
    [one] 1 allenamento
    [many] { $count } di allenamenti
   *[other] { $count } allenamenti
}

journal.cli.importer.desktop.exe.no.parent = l'eseguibile corrente non ha una directory superiore

journal.cli.importer.desktop.missing.sibling = eseguibile gemello mancante { $path }

journal.cli.importer.embedded.body.refusal = Le esportazioni di Apple Health e i dati di Oura vengono importati da un diario su un computer, quindi non è stato importato niente.

journal.cli.importer.error.apple.inspect = non è stato possibile esaminare l'esportazione di Apple Health; riprova con un'esportazione valida

journal.cli.importer.error.facet.not.unmuted = "{ $facet }" non è una delle tue facette non silenziate, quindi non è stato importato niente. le tue facette non silenziate: { $enabled }

journal.cli.importer.error.facet.scope = --facet si applica solo a un file audio, un file di testo semplice o un'importazione del corpo, quindi non è stato importato niente; importa questo senza

journal.cli.importer.error.facets.unreadable = non è stato possibile leggere le tue facette, quindi non è stato importato niente: { $error }

journal.cli.importer.error.no.facet.conflict = argomento --no-facet: non consentito con l'argomento --facet

journal.cli.importer.error.no.facet.scope = --no-facet si applica solo a un'importazione del corpo, quindi non è stato importato niente; importa questo senza

journal.cli.importer.error.preview.failed = non è stato possibile mostrare l'anteprima dell'importazione: { $error }

journal.cli.importer.error.stopped = importazione interrotta

journal.cli.importer.error.timestamp.format = il timestamp deve essere nel formato YYYYMMDD_HHMMSS

journal.cli.importer.error.timestamp.format.offset = il timestamp deve essere YYYYMMDD_HHMMSS, oppure una data e un'ora con il suo offset, come 2025-11-02T01:30:00-07:00

journal.cli.importer.error.unknown.source = fonte dell'importatore sconosciuta: { $source }

journal.cli.importer.error.zone.changed.preview = il fuso orario è cambiato dopo l'anteprima; mostra di nuovo l'anteprima prima di importare

journal.cli.importer.force.day.notice = avviso: questo file è datato { $fresh }, e un'importazione precedente ha usato { $stored }. questa importazione con --force userà { $planned }. le registrazioni esistenti non vengono spostate.

journal.cli.importer.force.pdf.day.notice = avviso: il nome di questo file è datato { $fresh }, e un'importazione precedente è stata annotata il { $stored }. le date proprie del PDF possono selezionare un giorno diverso. scegli --timestamp per impostarne la data. le registrazioni esistenti non vengono spostate.

journal.cli.importer.generic.text.complete = Importazione di testo generica completata: segments={ $segments }

journal.cli.importer.generic.text.untimed.line1 = nessun orario di turno trovato in questo file, quindi è arrivato come un segmento all'ora di inizio dell'importazione, ogni riga così com'è scritta.

journal.cli.importer.generic.text.untimed.line2 = se le tue trascrizioni segnano gli orari in un modo che il diario non legge ancora, chiedici di aggiungerlo: { $url }

journal.cli.importer.help.opt.archive.members = { $indent }Elenca i membri di un'esecuzione di archivio audio (da usare con --offset e --limit)

journal.cli.importer.help.opt.archive.status = { $indent }Stampa lo stato compatto di un'esecuzione di archivio audio

journal.cli.importer.help.opt.audio.archive.line1 = { $indent }--audio-archive PATH  Importa una cartella di registrazioni Ogg Opus o un manifest JSON

journal.cli.importer.help.opt.audio.archive.line2 = { $indent }Cartella: i nomi dei file Limitless e --source-zone sono obbligatori.

journal.cli.importer.help.opt.audio.archive.line3 = { $indent }Manifest: { $example }

journal.cli.importer.help.opt.audio.archive.line4 = { $indent }O usa local_civil e source_zone per l'orario di un membro.

journal.cli.importer.help.opt.audio.archive.line5 = { $indent }I percorsi sono assoluti o relativi al manifest.

journal.cli.importer.help.opt.audio.archive.line6 = { $indent }Gli orari espliciti del manifest hanno la precedenza sui nomi dei file.

journal.cli.importer.help.opt.audio.archive.line7 = { $indent }Supporta Ogg Opus mono/stereo a flusso singolo; gli altri contenitori sono esclusi.

journal.cli.importer.help.opt.audio.archive.line8 = { $indent }Mostra la dimensione prima di salvare; --dry-run non salva niente.

journal.cli.importer.help.opt.auto = { $indent }--auto [AUTO]         Adotta il timestamp rilevato; AUTO è una guida, non il percorso del file

journal.cli.importer.help.opt.backends = { $indent }--backends            Elenca i backend sincronizzabili dell'importatore

journal.cli.importer.help.opt.confirm.body = { $indent }--confirm-body-save   Conferma che questa esecuzione può salvare l'output sensibile dell'importatore del corpo

journal.cli.importer.help.opt.debug = { $indent }-d, --debug            Attiva il registro di debug

journal.cli.importer.help.opt.deterministic.line1 = { $indent }--deterministic-only  Quando non si può leggere una data dal file, saltalo (exit 0)

journal.cli.importer.help.opt.deterministic.line2 = { $indent }invece di fallire

journal.cli.importer.help.opt.dry.run = { $indent }--dry-run             Mostra che cosa verrebbe importato senza scrivere nel diario

journal.cli.importer.help.opt.facet.line1 = { $indent }--facet FACET         Archivia questo file audio o di testo, o questa importazione del corpo, sotto FACET,

journal.cli.importer.help.opt.facet.line2 = { $indent }una delle tue facette non silenziate, invece di lasciare che il tuo diario scelga

journal.cli.importer.help.opt.facet.line3 = { $indent }(per un'importazione del corpo: la tua facetta ricordata, o nessuna)

journal.cli.importer.help.opt.force = { $indent }--force               Forza la reimportazione; le fonti del corpo creano un'importazione separata

journal.cli.importer.help.opt.help = { $indent }-h, --help            mostra questo messaggio di aiuto ed esci

journal.cli.importer.help.opt.json.line1 = { $indent }--json                Scrivi i risultati come JSON; i salvataggi dell'archivio audio emettono oggetti di avanzamento

journal.cli.importer.help.opt.json.line2 = { $indent }e un risultato finale, un oggetto JSON per riga.

journal.cli.importer.help.opt.limit = { $indent }--limit LIMIT         Numero massimo di membri da elencare (predefinito 100, max 1000)

journal.cli.importer.help.opt.list.importers = { $indent }--list-importers      Elenca gli importatori di file disponibili

journal.cli.importer.help.opt.no.facet.line1 = { $indent }--no-facet            Archivia questa importazione del corpo senza facetta, qualunque sia il predefinito

journal.cli.importer.help.opt.no.facet.line2 = { $indent }ricordato

journal.cli.importer.help.opt.offset = { $indent }--offset OFFSET       Scostamento iniziale del membro per l'elenco dei membri

journal.cli.importer.help.opt.resume = { $indent }--resume IMPORT_ID    Completa le parti non salvate del piano congelato di un archivio compresso audio

journal.cli.importer.help.opt.save = { $indent }--save                Con --sync: scarica e importa i file nuovi (il predefinito è il dry-run)

journal.cli.importer.help.opt.source = { $indent }--source SOURCE       Obbligatorio per i formati classificati (ics, esportazione di chat, immagine, PDF)

journal.cli.importer.help.opt.source.zone = { $indent }--source-zone IANA    Fuso orario per gli archivi compressi audio in cartella o per le ore civili (es. America/New_York)

journal.cli.importer.help.opt.timestamp = { $indent }I .txt/.md generici usano questo, o --auto per adottare un timestamp rilevato (--timestamp YYYYMMDD_HHMMSS, o una data e un'ora con il suo scostamento, come 2025-11-02T01:30:00-07:00)

journal.cli.importer.help.opt.verbose = { $indent }-v, --verbose         Attiva l'output dettagliato

journal.cli.importer.help.options.header = opzioni:

journal.cli.importer.help.positional.header = argomenti posizionali:

journal.cli.importer.help.positional.media = { $indent }media                 Percorso di un file audio o di testo

journal.cli.importer.help.summary = Importa un file di contenuti multimediali nel diario

journal.cli.importer.help.usage = uso: solstone journal importer [-h] [--timestamp TIMESTAMP] [--facet FACET] [--no-facet]

journal.cli.importer.list.desc.apple.health = Mostra l'anteprima dei dati export.xml di Apple Health senza scrivere nel diario

journal.cli.importer.list.desc.chatgpt = Importa le conversazioni da un'esportazione di ChatGPT

journal.cli.importer.list.desc.claude = Importa le conversazioni da un'esportazione di chat di Claude

journal.cli.importer.list.desc.document = Importa documenti PDF con estrazione di testo e raster tramite worker

journal.cli.importer.list.desc.evernote = Note da un'esportazione di Evernote

journal.cli.importer.list.desc.gemini = Importa le conversazioni da un'esportazione Gemini o Bard di Google Takeout

journal.cli.importer.list.desc.ics = Importa eventi da file di calendario ICS o da uno ZIP di esportazione di Google Calendar

journal.cli.importer.list.desc.image = Importa una sola immagine e descrivi i suoi contenuti con la visione

journal.cli.importer.list.desc.journal.archive = Unisci un archivio compresso di diario esportato nel diario attuale

journal.cli.importer.list.desc.oura = Mostra l'anteprima dei documenti JSON dell'API v2 di Oura (fixture sintetiche; il percorso di salvataggio è una fase successiva e condizionata)

journal.cli.importer.list.desc.strava = Allenamenti da uno scaricamento di Strava (si legge solo il suo activities.csv)

journal.cli.importer.list.header = Importatori di file:

journal.cli.importer.notes.folder.retired = l'importazione di una cartella di note non c'è più; un vault di Obsidian arriva nel tuo diario quando lo attivi nell'app di solstone sul computer dove vive il vault

journal.cli.importer.oura.app.needed = questo diario non è ancora connesso a un'app che hai registrato con Oura. registrane una, poi esegui `solstone journal importer --connect oura` in un terminale e incolla il suo ID client e il suo segreto client. come: { $how }

journal.cli.importer.oura.app.prompt.client.id = ID client:

journal.cli.importer.oura.app.prompt.client.secret = segreto client:

journal.cli.importer.oura.app.prompt.intro = questo diario non è ancora connesso a un'app che hai registrato con Oura. per connetterne una:

journal.cli.importer.oura.app.prompt.note = sono conservati nelle impostazioni di questo diario e si usano solo per connettersi a Oura. non si salva niente finché Oura non dice sì. come: { $how }

journal.cli.importer.oura.app.prompt.step1 = { $indent }1. accedi a https://cloud.ouraring.com/oauth/applications e crea un'app

journal.cli.importer.oura.app.prompt.step2 = { $indent }2. aggiungi l'URI di reindirizzamento http://localhost:8765/callback

journal.cli.importer.oura.app.prompt.step3 = { $indent }3. incolla sotto l'ID client e il segreto client dell'app

journal.cli.importer.oura.connect.failed.browser = non è stato possibile aprire un browser qui. eseguilo sul computer del diario, non in una sessione remota.

journal.cli.importer.oura.connect.failed.callback = L'accesso di Oura torna su questo computer alla porta 8765, e qualcos'altro la sta usando. chiudi quello e rieseguilo.

journal.cli.importer.oura.connect.failed.declined = Oura non si è connesso, quindi non è stato salvato niente.

journal.cli.importer.oura.connect.failed.exchange = Oura non ha accettato la connessione, quindi non è stato salvato niente. controlla il segreto client nella pagina della tua app su Oura e rieseguilo.

journal.cli.importer.oura.connect.failed.timeout = L'accesso di Oura non è finito entro cinque minuti, quindi non è stato salvato niente. rieseguilo per riprovare.

journal.cli.importer.plaud.begin.invalid.redirect = URI di reindirizzamento del callback di Plaud non valido

journal.cli.importer.plaud.begin.loopback.bind = non è stato possibile riservare il listener locale di callback

journal.cli.importer.plaud.begin.random.failed = non è stato possibile generare l'identificatore di accesso

journal.cli.importer.plaud.connect.detail.busy = un altro accesso o un'altra sincronizzazione è già in esecuzione

journal.cli.importer.plaud.connect.detail.callback.invalid = l'indirizzo di accesso non era valido

journal.cli.importer.plaud.connect.detail.cancelled = accesso annullato

journal.cli.importer.plaud.connect.detail.denied = l'accesso non è stato approvato

journal.cli.importer.plaud.connect.detail.exchange.failed = non è stato possibile finire di accedere

journal.cli.importer.plaud.connect.detail.expired = il tentativo di accesso è scaduto

journal.cli.importer.plaud.connect.detail.storage = non è stato possibile salvare l'accesso in questo diario

journal.cli.importer.plaud.connect.done = { $provider } è connesso, e il tuo accesso è salvato in questo diario.

journal.cli.importer.plaud.connect.done.account = { $provider } è connesso come { $account }, e il tuo accesso è salvato in questo diario.

journal.cli.importer.plaud.connect.failed = non è stato possibile connettere { $provider }: { $detail }

journal.cli.importer.plaud.connect.next = esegui { $command } per far arrivare le registrazioni ora, oppure attiva la sincronizzazione oraria nelle impostazioni.

journal.cli.importer.plaud.connect.open = apri questo link per accedere con { $provider }:

journal.cli.importer.plaud.connect.retry.url = quello non era l'indirizzo di accesso di { $provider }. incolla l'indirizzo completo dalla barra del browser, oppure finisci nel browser.

journal.cli.importer.plaud.connect.waiting = in attesa che { $provider } finisca l'accesso. se il tuo browser è su un altro dispositivo, incolla l'indirizzo finale dalla barra del browser e premi invio:

journal.cli.importer.plaud.failure.audio.not.ready = il link di scaricamento dell'audio non è ancora pronto; si riproverà alla prossima sincronizzazione

journal.cli.importer.plaud.failure.catalogue = catalogo non riuscito

journal.cli.importer.plaud.failure.download = scaricamento non riuscito

journal.cli.importer.plaud.failure.manifest = corrispondenza dell'importazione non riuscita

journal.cli.importer.plaud.failure.pipeline = importazione non riuscita

journal.cli.importer.plaud.failure.temporary.url = non è stato possibile ottenere l'URL di scaricamento

journal.cli.importer.plaud.failure.token.refused = l'accesso è scaduto o è stato revocato; accedi di nuovo nelle impostazioni

journal.cli.importer.plaud.preview.complete = Anteprima della sincronizzazione Plaud completa: files={ $files }

journal.cli.importer.plaud.sync.busy = il tuo diario sta già controllando { $provider }. riprova quando finisce.

journal.cli.importer.plaud.sync.missing.credential = La credenziale di Plaud non è configurata

journal.cli.importer.plaud.sync.operation = Plaud { $detail }

journal.cli.importer.plaud.sync.state.failed = Stato della sincronizzazione Plaud non riuscito: { $message }

journal.cli.importer.preview.chatgpt = { $messages } dall'esportazione di ChatGPT

journal.cli.importer.preview.claude = { $messages } dall'esportazione di chat di Claude

journal.cli.importer.preview.document.counts = { $documents }, { $pages }

journal.cli.importer.preview.document.none = Nessun documento PDF trovato

journal.cli.importer.preview.document.unreadable = { $documents }, { $pages }; { $unreadable } illeggibili ({ $failures })

journal.cli.importer.preview.gemini = { $messages } dall'esportazione di Gemini

journal.cli.importer.preview.ics.counts = { $events }, { $attendees }

journal.cli.importer.preview.ics.no.events = Nessun evento trovato nei dati ICS

journal.cli.importer.preview.ics.none = Nessun dato ICS trovato

journal.cli.importer.preview.image.one = 1 immagine ({ $format }, { $width }×{ $height })

journal.cli.importer.registry.archive.error.and.record.failed = importazione di { $source } non riuscita: { $error }; non è stato possibile annotare il tentativo: { $record_error }

journal.cli.importer.registry.archive.missing.facts = importazione di { $source } non riuscita: fatti del tentativo mancanti per { $attempt }

journal.cli.importer.registry.archive.missing.provenance = importazione di { $source } non riuscita: provenienza mancante per il tentativo { $attempt }

journal.cli.importer.registry.archive.record.attempt.failed = importazione di { $source } non riuscita: non è stato possibile annotare il tentativo: { $error }

journal.cli.importer.registry.archive.record.completed.failed = importazione di { $source } non riuscita: non è stato possibile annotare il tentativo completato: { $error }

journal.cli.importer.registry.archive.record.merge.failed = importazione di { $source } non riuscita: non è stato possibile annotare i risultati dell'unione: { $error }

journal.cli.importer.registry.attempt.superseded = importazione di { $source } non riuscita: il tentativo { $import_id }:{ $generation } è stato sostituito

journal.cli.importer.registry.evernote.attachments.failed = Importazione di Evernote non riuscita. { $error }

journal.cli.importer.registry.found.nothing = importazione di { $source } non riuscita: non si è trovato niente da importare

journal.cli.importer.registry.gemini.folder.unreadable = Importazione di Gemini non riuscita. non è stato possibile leggere la cartella di esportazione salvata.

journal.cli.importer.registry.originals.choice.unreadable = non è stato possibile leggere la scelta di conservare gli originali. riapri l'importazione per riprovare.

journal.cli.importer.registry.originals.cleanup.failed = salvato nel tuo diario. non è stato possibile rimuovere la copia extra.

journal.cli.importer.registry.originals.keep.failed = non è stato possibile conservare una copia dell'originale. riprova.

journal.cli.importer.registry.preview.failed = anteprima dell'importazione non riuscita: { $error }

journal.cli.importer.registry.strava.discard.failed = non è stato possibile rimuovere lo scaricamento di Strava preparato: { $error }

journal.cli.importer.registry.strava.filing.failed = importazione di { $source } non riuscita: non è stato possibile archiviare gli allenamenti: { $error }

journal.cli.importer.registry.strava.release.failed = non è stato possibile rilasciare gli allenamenti collocati all'ora sbagliata

journal.cli.importer.registry.summary.image = importata 1 immagine

journal.cli.importer.registry.summary.pdf = importati { $documents }

journal.cli.importer.registry.unconfirmed = l'importazione di { $source } è stata salvata nel tuo diario, ma non è stato possibile confermare che gli elementi siano pronti per la ricerca; rieseguila

journal.cli.importer.size.listed.moves = { $quantity ->
    [one] 1 allenamento già elencato nella pagina delle attività passerebbe alla facetta di questa importazione da { $from }
    [many] { $count } di allenamenti già elencati nella pagina delle attività passerebbero alla facetta di questa importazione da { $from }
   *[other] { $count } allenamenti già elencati nella pagina delle attività passerebbero alla facetta di questa importazione da { $from }
}

journal.cli.importer.size.recordings.across = { $recordings } nell'arco di { $days }

journal.cli.importer.size.skipped = { $count } omessi

journal.cli.importer.skipped.already = Importazione omessa: questo file è già stato importato; usa --force per importarlo di nuovo

journal.cli.importer.skipped.confirm.date = Importazione omessa: la data del file va confermata; rieseguilo con --timestamp o --auto

journal.cli.importer.skipped.no.date = Importazione omessa: non è stato possibile leggere una data dal file; indicane una con --timestamp YYYYMMDD_HHMMSS

journal.cli.importer.source.archive.already.present = l'importazione di { $source } non ha unito niente: il contenuto dell'archivio compresso è già presente

journal.cli.importer.source.archive.complete = importazione di { $source } completa: segments_copied={ $segments_copied } imports_copied={ $imports_copied } entities_created={ $entities_created } entities_merged={ $entities_merged } facets_created={ $facets_created } facets_merged={ $facets_merged }

journal.cli.importer.source.archive.incomplete = importazione di { $source } incompleta: { $detail }

journal.cli.importer.source.complete = importazione di { $source } completa: entries_written={ $entries } files_created={ $files } errors={ $errors } summary={ $summary }

journal.cli.importer.source.failed = importazione di { $source } non riuscita: { $detail }

journal.cli.importer.source.preview = anteprima di { $source }: date_range={ $start }..{ $end } items={ $items } entities={ $entities } summary={ $summary }

journal.cli.importer.summary.archive = esportazione del diario: { $days }, { $entities }, { $facets } ({ $warnings })

journal.cli.importer.summary.conversations = importati { $messages } da { $conversations } nell'arco di { $days }

journal.cli.importer.summary.document = importati { $documents } nell'arco di { $days } in { $segments }

journal.cli.importer.summary.ics.base = importati { $events } nell'arco di { $days }

journal.cli.importer.summary.ics.over.limit = ; { $count } non elencati per il giorno in cui avvengono, oltre le { $limit } date che un'importazione elenca

journal.cli.importer.summary.ics.reason.invalid = { $count } illeggibili

journal.cli.importer.summary.ics.reason.no.change = { $count } senza orario di modifica

journal.cli.importer.summary.ics.reason.unknown.zone = { $count } in un fuso orario sconosciuto

journal.cli.importer.summary.ics.skipped = ; { $skipped } non elencati per il giorno in cui avvengono ({ $reasons })

journal.cli.importer.summary.ics.truncated.series = { $quantity ->
    [one] ; 1 serie ripetuta elencata solo per le sue ultime { $limit } date
    [many] ; { $count } di serie ripetute elencate solo per le loro ultime { $limit } date
   *[other] ; { $count } serie ripetute elencate solo per le loro ultime { $limit } date
}

journal.cli.importer.summary.ics.unexpandable.series = { $quantity ->
    [one] ; 1 serie ripetuta elencata solo per la sua prima data
    [many] ; { $count } di serie ripetute elencate solo per la loro prima data
   *[other] ; { $count } serie ripetute elencate solo per la loro prima data
}

journal.cli.importer.summary.image.none = Nessuna immagine leggibile trovata

journal.cli.importer.sync.available = Backend disponibili: { $backends }

journal.cli.importer.sync.complete = sincronizzazione di { $backend } completa: saved={ $saved } errors={ $errors }

journal.cli.importer.sync.unknown.backend = Backend di sincronizzazione sconosciuto: { $backend }

journal.cli.importer.text.failed = importazione di testo non riuscita: { $error }

journal.cli.importer.text.unconfirmed = l'importazione di testo è stata salvata nel tuo diario, ma non è stato possibile confermare che gli elementi siano pronti per la ricerca; rieseguila

journal.cli.importer.timestamp.confirm = timestamp rilevato { $timestamp }; rieseguilo con --timestamp { $timestamp } o --auto

journal.cli.oauth.callback.failure = l'accesso non è finito. torna al tuo diario e riprova

journal.cli.oauth.callback.success = hai effettuato l'accesso. puoi chiudere questa scheda e tornare al tuo diario

journal.cli.oauth.oura.callback.allowed = l'hai consentito su Oura. puoi chiudere questa scheda; il terminale dice quando è connesso.

journal.cli.oauth.oura.callback.declined = Oura non è stato connesso. puoi chiudere questa scheda.

journal.cli.oauth.oura.callback.rejected = Autorizzazione di Oura rifiutata.

journal.cli.setup.clean.changed.during.service = l'installazione è cambiata mentre il suo servizio veniva rimosso

journal.cli.setup.clean.child.exited = la disinstallazione del servizio è uscita con { $code }

journal.cli.setup.clean.child.exited.detail = la disinstallazione del servizio è uscita con { $code }: { $detail }

journal.cli.setup.clean.child.timed.out = la disinstallazione del servizio è uscita con { $code } (tempo scaduto)

journal.cli.setup.clean.child.timed.out.detail = la disinstallazione del servizio è uscita con { $code } (tempo scaduto): { $detail }

journal.cli.setup.clean.config.invalid.toml = la configurazione non è un TOML valido

journal.cli.setup.clean.config.io = non è stato possibile leggere la configurazione: { $error }

journal.cli.setup.clean.config.missing.journal = la configurazione non ha la chiave journal

journal.cli.setup.clean.config.not.absolute = il percorso journal della configurazione non è assoluto

journal.cli.setup.clean.config.not.string = il valore journal della configurazione non è una stringa

journal.cli.setup.clean.confirm.config = { $indent }[{ $marker }] configurazione: { $path }

journal.cli.setup.clean.confirm.header = solstone journal setup --clean-uninstall rimuoverà questi artefatti dell'ambiente di esecuzione:

journal.cli.setup.clean.confirm.keep.header = non rimuoverà:

journal.cli.setup.clean.confirm.keep.journal = { $indent }- directory del diario: { $path }

journal.cli.setup.clean.confirm.keep.macos.app = { $indent }- /Applications/solstone.app

journal.cli.setup.clean.confirm.keep.macos.permissions = { $indent }- permessi del microfono o della registrazione dello schermo di macOS

journal.cli.setup.clean.confirm.keep.macos.support = { $indent }- ~/Library/Application Support/solstone/

journal.cli.setup.clean.confirm.keep.python = { $indent }- un'installazione residua di journal con pip, uv o pipx

journal.cli.setup.clean.confirm.manifest = { $indent }[{ $marker }] manifest del diario: { $path }

journal.cli.setup.clean.confirm.marker.absent = assente

journal.cli.setup.clean.confirm.marker.present = presente

journal.cli.setup.clean.confirm.marker.retain = da conservare

journal.cli.setup.clean.confirm.service = { $indent }[{ $marker }] servizio: { $path }

journal.cli.setup.clean.confirm.service.unavailable = { $indent }posizione del servizio non disponibile: { $error }

journal.cli.setup.clean.confirm.wrapper = { $indent }[{ $marker }] involucro: { $path }

journal.cli.setup.clean.journal.line = diario: { $path }

journal.cli.setup.clean.no.binding = disinstallazione pulita rifiutata: nessuna identità di installazione adottata

journal.cli.setup.clean.no.owned.path = nessun percorso proprio

journal.cli.setup.clean.outcome.already.complete = la disinstallazione pulita è già completa

journal.cli.setup.clean.outcome.cancelled = annullato

journal.cli.setup.clean.outcome.complete = disinstallazione pulita completa: { $removed } rimossi, { $already_absent } già assenti, { $preserved } conservati, { $skipped } omessi, { $failed } non riusciti

journal.cli.setup.clean.outcome.no.eligible = nessun file idoneo della configurazione iniziale da rimuovere

journal.cli.setup.clean.outcome.not.tty = non è un tty; riesegui con --yes per procedere senza interazione (annullato)

journal.cli.setup.clean.outcome.service.unavailable = posizione del servizio non disponibile: { $error }

journal.cli.setup.clean.outcome.stopped.failed = la disinstallazione pulita si è fermata dopo un passo non riuscito

journal.cli.setup.clean.preflight.census.error = il censimento dell'identità di installazione è illeggibile: { $error }

journal.cli.setup.clean.preflight.census.unreadable = il censimento dell'identità di installazione è illeggibile

journal.cli.setup.clean.preflight.config.mismatch = il journal della configurazione non corrisponde all'identità di installazione

journal.cli.setup.clean.preflight.failed = controllo preliminare della disinstallazione pulita non riuscito: { $error }

journal.cli.setup.clean.preflight.overlap = l'archiviazione dell'identità si sovrappone a un diario protetto

journal.cli.setup.clean.preflight.prepared = l'identità di installazione è preparata

journal.cli.setup.clean.readmit.failed = non è stato possibile riammettere l'installazione dopo la rimozione del suo servizio: { $error }

journal.cli.setup.clean.reason.manifest.preserved = il manifest del diario è conservato

journal.cli.setup.clean.reason.no.owned.path.known = non si conosce alcun percorso proprio

journal.cli.setup.clean.reason.no.owned.path.platform = nessun percorso proprio su questa piattaforma

journal.cli.setup.clean.reason.preserved.mark = conservato: { $mark }

journal.cli.setup.clean.reason.preserved.non.dir = destinazione conservata che non è una directory

journal.cli.setup.clean.reason.preserved.symlink = collegamento simbolico conservato

journal.cli.setup.clean.reason.service.guard.mismatch = la guardia del servizio o dell'involucro non corrisponde a questa installazione, non si rimuove

journal.cli.setup.clean.reason.skill.foreign = la destinazione del collegamento simbolico è estranea

journal.cli.setup.clean.reason.skill.preserved = il contenuto delle tue skill è conservato

journal.cli.setup.clean.reason.skipped.after.failure = il passo non è stato eseguito dopo un fallimento precedente

journal.cli.setup.clean.reason.wrapper.cross.repo = l'alias punta a { $target }, non si rimuove

journal.cli.setup.clean.reason.wrapper.dangling = l'alias è pendente (manca la destinazione { $target }), non si rimuove

journal.cli.setup.clean.reason.wrapper.foreign = l'alias non è un collegamento simbolico gestito, non si rimuove

journal.cli.setup.clean.reason.wrapper.guard.mismatch = la guardia dell'involucro non corrisponde a questa installazione, non si rimuove

journal.cli.setup.clean.reason.wrapper.unexpected = stato dell'alias imprevisto: { $state }

journal.cli.setup.clean.reason.wrapper.unknown = sconosciuto

journal.cli.setup.clean.reason.wrapper.worktree = si rifiuta di agire da un worktree di git

journal.cli.setup.clean.refusal.incompatible = --clean-uninstall non può essere combinato con { $flags }

journal.cli.setup.clean.refusal.jsonl = JSONL non è supportato come output per --clean-uninstall in questa versione.

journal.cli.setup.clean.refused = disinstallazione pulita rifiutata: { $reason }

journal.cli.setup.clean.step.running = [passo { $step }/{ $total }] disinstallazione di { $name } in corso...

journal.cli.setup.error.home.dir = configurazione iniziale: non è stato possibile determinare la tua directory home

journal.cli.setup.error.refused.before.mutation = solstone journal setup: rifiutato prima della modifica: { $error }

journal.cli.setup.error.sign.in.windows.only = solstone journal setup: --initial-start-at-sign-in è disponibile solo su Windows

journal.cli.setup.host.journal.binding.changed = l'associazione del diario è cambiata

journal.cli.setup.host.packaged.speech.unavailable = prova del parlato pacchettizzata non disponibile

journal.cli.setup.identity.adopted.no.marker = il registro adottato non ha un marcatore di adozione

journal.cli.setup.identity.changed.retry = l'installazione è cambiata durante il cambio della cartella del suo diario; riprova

journal.cli.setup.identity.executable.unavailable = l'eseguibile attuale non è disponibile

journal.cli.setup.identity.finish.recovery.first = finisci la configurazione iniziale o il ripristino prima di cambiare la cartella del diario

journal.cli.setup.identity.finish.setup.first = finisci la configurazione iniziale del diario prima di cambiare la sua cartella

journal.cli.setup.identity.foreign.artifacts = gli artefatti esistenti di involucro o di servizio sono legati a un'installazione diversa

journal.cli.setup.identity.generation.decimal = la generazione non è un decimale canonico

journal.cli.setup.identity.generation.nonzero = la generazione deve essere diversa da zero

journal.cli.setup.identity.generation.overflow = overflow della generazione

journal.cli.setup.identity.generation.range = la generazione è fuori intervallo

journal.cli.setup.identity.guard.duplicate = guardia di identità duplicata { $field }

journal.cli.setup.identity.guard.incomplete = la guardia dell'artefatto non è un'identità corrispondente completa

journal.cli.setup.identity.guard.invalid = guardia di identità non valida { $field }

journal.cli.setup.identity.guard.mismatch = la guardia dell'artefatto non corrisponde all'identità

journal.cli.setup.identity.guard.missing = i campi della guardia di identità sono incompleti

journal.cli.setup.identity.guard.root.mismatch = la guardia dell'artefatto non corrisponde alla radice ammessa

journal.cli.setup.identity.guarded.no.record = gli artefatti esistenti di involucro o di servizio sono sotto guardia, ma nell'archiviazione dell'identità non esiste un registro di installazione corrispondente

journal.cli.setup.identity.home.unset = HOME non è impostato

journal.cli.setup.identity.io.create.home = creare la directory home della configurazione iniziale: { $error }

journal.cli.setup.identity.legacy.manifest.invalid = la prova del manifest legacy è malformata o illeggibile

journal.cli.setup.identity.legacy.needs.manifest = gli artefatti legacy richiedono un manifest schema-v1 valido senza fornitore

journal.cli.setup.identity.namespace.mismatch = il registro dello spazio dei nomi non corrisponde al suo spazio dei nomi radice

journal.cli.setup.identity.no.bootstrap.evidence = gli artefatti esistenti non hanno una prova di bootstrap valida

journal.cli.setup.identity.not.found = l'identità di installazione non esiste

journal.cli.setup.identity.owner.root.changed = la radice di ammissione del titolare è cambiata durante il blocco

journal.cli.setup.identity.path.outside = PATH si risolve fuori dall'installazione V2

journal.cli.setup.identity.prepared.uninstall = l'identità preparata non può essere disinstallata

journal.cli.setup.identity.provider.app.dir = la base del fornitore di installazione non ha una directory dell'applicazione

journal.cli.setup.identity.provider.parent = la base del fornitore di installazione non ha una directory superiore

journal.cli.setup.identity.record.stage.malformed = fase del registro malformata

journal.cli.setup.identity.record.stage.stale = la fase del registro non aggiornata non corrisponde al suo nome

journal.cli.setup.identity.root.unavailable = la radice di installazione non è disponibile

journal.cli.setup.identity.tombstoned.no.marker = il registro con lapide non ha un marcatore di adozione

journal.cli.setup.identity.uncertain.artifacts = l'associazione dell'artefatto è malformata o ambigua

journal.cli.setup.identity.windows.path.odd = Il percorso di Windows ha un numero dispari di byte

journal.cli.setup.identity.windows.platform.mismatch = la piattaforma del titolare non corrisponde a questa compilazione di Windows

journal.cli.setup.identity.wrapper.journal.unsafe = il percorso effettivo del diario non può essere rappresentato in modo sicuro negli involucri gestiti

journal.cli.setup.narrate.artifacts.header = artefatti:

journal.cli.setup.narrate.artifacts.none = { $indent }nessuno

journal.cli.setup.narrate.close.finish.unix = finisci la configurazione iniziale: solstone journal open /init

journal.cli.setup.narrate.close.finish.url = finisci la configurazione iniziale: { $url }/init

journal.cli.setup.narrate.close.open.unix = apri il tuo diario: solstone journal open

journal.cli.setup.narrate.close.open.url = apri il tuo diario: { $url }

journal.cli.setup.narrate.close.skip.service.unix = una volta che il tuo diario è in esecuzione, aprilo con: solstone journal open

journal.cli.setup.narrate.close.skip.service.url = una volta che il tuo diario è in esecuzione, aprilo su { $url }

journal.cli.setup.narrate.complete.counts = { $skipped } di { $total } passi già fatti; eseguiti { $ran }

journal.cli.setup.narrate.complete.installed = solstone è installato.

journal.cli.setup.narrate.complete.set.up = solstone è configurato.

journal.cli.setup.narrate.model.recovery.direct = l'installazione del modello non è terminata. esegui { $journal } install-models --variant { $variant }, poi { $journal } setup.

journal.cli.setup.narrate.model.recovery.installer = l'installazione del modello non è terminata. esegui { $journal } install-models --variant { $variant }.

journal.cli.setup.narrate.prior.clean.force = solstone journal setup è stato eseguito senza errori l'ultima volta il { $completed_at }; nuova esecuzione di tutti i passi (--force).

journal.cli.setup.narrate.prior.clean.verify = solstone journal setup è stato eseguito senza errori l'ultima volta il { $completed_at }; verifica dello stato attuale.

journal.cli.setup.narrate.prior.force.hint = Usa --force per eseguire di nuovo tutti i passi senza condizioni.

journal.cli.setup.narrate.prior.incomplete.header = l'ultima esecuzione di solstone journal setup il { $started_at } ha lasciato questi passi incompleti:

journal.cli.setup.narrate.prior.incomplete.hint = Una nuova esecuzione verificherà lo stato e ripeterà i passi incompleti.

journal.cli.setup.narrate.prior.incomplete.item = { $indent }- { $name } (non riuscito)

journal.cli.setup.narrate.sol.keeps.journal = solstone su questo Mac tiene già questo diario, quindi la configurazione iniziale non ha installato un lanciatore in secondo piano.{ $line_break }Esegui solstone journal doctor se qualcosa non va.

journal.cli.setup.narrate.step.failed = solstone journal setup: { $name } non riuscito: { $message }

journal.cli.setup.narrate.step.running = [passo { $index }/{ $total }] { $name } in esecuzione...

journal.cli.setup.narrate.step.running.command = [passo { $index }/{ $total }] { $name } in esecuzione: { $command }

journal.cli.setup.narrate.step.skipped = [passo { $index }/{ $total }] { $name } saltato: { $reason }

journal.cli.setup.parser.cwd = non è stato possibile determinare la directory corrente: { $error }

journal.cli.setup.parser.path.empty = { $journal_flag } non deve essere vuoto

journal.cli.setup.parser.path.encoding = non è stato possibile risolvere { $journal_flag }: il percorso non è { $encoding }

journal.cli.setup.parser.path.resolve = non è stato possibile risolvere { $journal_flag }: { $error }

journal.cli.setup.parser.port = { $port_flag } deve essere compreso in { $range } (ricevuto { $value })

journal.cli.setup.parser.sign.in.encoding = argomento { $sign_in_flag }: atteso { $on_value } o { $off_value }

journal.cli.setup.parser.sign.in.invalid = argomento { $sign_in_flag }: scelta non valida: { $value } (scegli tra { $choices })

journal.cli.setup.parser.sign.in.repeated = argomento { $sign_in_flag }: non deve essere ripetuto

journal.cli.setup.parser.timeout = argomento { $timeout_flag }: valore int non valido

journal.cli.setup.parser.unknown = argomenti non riconosciuti

journal.cli.setup.parser.variant = argomento { $variant_flag }: scelta non valida: { $value } (scegli tra { $choices })

journal.cli.setup.path.added = percorso: aggiunto ~/.local/bin al PATH della shell

journal.cli.setup.path.added.restart = percorso: aggiunto ~/.local/bin al PATH della shell — riavvia la shell o esegui 'exec $SHELL -l' per applicarlo

journal.cli.setup.path.already = percorso: ~/.local/bin è già nel PATH

journal.cli.setup.path.manual = percorso: non è stato possibile aggiungere in automatico ~/.local/bin al PATH — aggiungi a mano questa riga nel rc della shell: export PATH="$HOME/.local/bin:$PATH"

journal.cli.setup.path.windows = I pacchetti Windows espongono solstone direttamente; non è stato installato alcun involucro POSIX del PATH.

journal.cli.setup.plan.brain = imposterebbe la corsia del fornitore locale se questo computer può eseguire il modello locale, e lo inizializzerebbe se niente lo blocca

journal.cli.setup.plan.doctor = eseguirebbe: { $binary } journal doctor --readiness

journal.cli.setup.plan.heading.dry.run = dry-run della configurazione iniziale:

journal.cli.setup.plan.heading.explain = piano della configurazione iniziale:

journal.cli.setup.plan.identity.admission = ammissione dell'identità: verrebbe eseguita prima dei passi di modifica della configurazione iniziale

journal.cli.setup.plan.install.models = eseguirebbe: { $binary } journal install-models --variant { $variant }

journal.cli.setup.plan.journal = { $indent }diario: { $path } ({ $source })

journal.cli.setup.plan.mode = { $indent }modalità: { $mode }

journal.cli.setup.plan.model.download.default = { $indent }download del modello: nessun limite di tempo della configurazione iniziale (predefinito)

journal.cli.setup.plan.port = { $indent }porta: { $value } ({ $source })

journal.cli.setup.plan.service = installerebbe e avvierebbe il servizio del diario

journal.cli.setup.plan.skills.journal = eseguirebbe: { $binary } skills install --project { $journal } --agent all

journal.cli.setup.plan.skills.user = eseguirebbe: { $binary } skills install --agent all

journal.cli.setup.plan.source.checkout.false = { $indent }checkout dei sorgenti: False

journal.cli.setup.plan.source.checkout.true = { $indent }checkout dei sorgenti: True

journal.cli.setup.plan.step.header = [passo { $index }/{ $total }] { $name }

journal.cli.setup.plan.step.timeout = { $indent }step_timeout_seconds: { $value } ({ $source })

journal.cli.setup.plan.variant = { $indent }variante: { $value } ({ $source })

journal.cli.setup.plan.wrapper.unix = predisporrebbe nel processo gli involucri gestiti solstone e journal

journal.cli.setup.plan.wrapper.windows = salterebbe la predisposizione dell'involucro POSIX; il pacchetto Windows espone i comandi direttamente

journal.cli.setup.plan.write.config = scriverebbe: { $path }

journal.cli.setup.prompt.existing.journal = { $path } contiene già dati del diario; procedi? [y/N]:

journal.cli.setup.prompt.proceed = procedi? [y/N]:

journal.cli.setup.service.restart.exited = il riavvio del servizio è uscito con { $code }

journal.cli.setup.warning.launcher.ownership = avviso: non è stato possibile esaminare la proprietà del lanciatore di solstone: { $error }

journal.cli.setup.warning.manifest = avviso: non è stato possibile scrivere il manifest della configurazione iniziale: { $error }

journal.cli.setup.warning.package.receipt = solstone journal setup: avviso: la ricevuta del pacchetto non è stata scritta: { $error }

journal.cli.setup.wrapper.retired.note = rimossi gli strumenti da riga di comando della versione precedente; i loro backup sono in ~/.local/share/solstone/setup-backups

journal.cli.setup.wrapper.skip.path = lasciati invariati i file di login della shell perché è stato selezionato --skip-path

journal.cli.think.activity.blocked = attività bloccata

journal.cli.think.activity.body.data = questa attività proviene da un allenamento { $provider }, che è un dato del corpo; non viene eseguito nient'altro su di esso.

journal.cli.think.activity.day.missing = manca il file del giorno dell'attività: { $path }

journal.cli.think.activity.fingerprint.failed = non è possibile calcolare l'impronta degli input dell'attività

journal.cli.think.activity.malformed = la dichiarazione della facetta è malformata. correggila, poi riprova con { $reactivate_flag }.

journal.cli.think.activity.missing = manca la facetta di destinazione. dopo aver ripristinato la sua dichiarazione, riprova con { $reactivate_flag }; altrimenti riclassifica il segmento di origine.

journal.cli.think.activity.muted.blocked = la facetta di destinazione è silenziata. riattivala, poi riprova l'attività bloccata con { $reactivate_flag }.

journal.cli.think.activity.muted.retry = la facetta di destinazione è silenziata. riattivala per riprendere l'elaborazione dell'attività non finita.

journal.cli.think.activity.owner.field.held = l'output dell'attività è trattenuto da te

journal.cli.think.activity.pending = nuovo tentativo dell'attività in attesa

journal.cli.think.activity.read.blocked = non è stato possibile leggere la dichiarazione della facetta. dopo aver ripristinato l'accesso, riprova l'attività bloccata con { $reactivate_flag }.

journal.cli.think.activity.read.retry = non è stato possibile leggere la dichiarazione della facetta. l'attività non finita riproverà automaticamente.

journal.cli.think.activity.record.missing = non esiste un registro dell'attività per questa classificazione. dopo aver corretto la dichiarazione, rielabora il segmento di origine.

journal.cli.think.activity.repair.required = richiede una riparazione

journal.cli.think.activity.replaced = la facetta di destinazione è stata sostituita. riclassifica il segmento di origine; un risultato vecchio non può essere applicato al sostituto.

journal.cli.think.activity.retry.identity = mancata corrispondenza dell'identità del lavoro dell'attività

journal.cli.think.activity.retry.kind = non è un lavoro dell'attività

journal.cli.think.activity.retry.muted = la facetta dell'attività è silenziata

journal.cli.think.activity.routing = correggi la dichiarazione, poi rielabora questo segmento di origine.

journal.cli.think.activity.row.missing = manca la riga dell'attività: { $path }

journal.cli.think.activity.settled = { $facet }/{ $activity } conclusa: questa attività proviene da un allenamento { $provider }, che è un dato del corpo; non viene eseguito nient'altro su di esso.

journal.cli.think.activity.state = stato dell'attività non salvato: { $error }

journal.cli.think.activity.state.segment = { $segment }: stato dell'attività non salvato: { $error }

journal.cli.think.activity.work.malformed = lavoro dell'attività malformato: { $path }

journal.cli.think.activity.work.pending = lavoro dell'attività in attesa: { $failures }

journal.cli.think.activity.work.running = l'input precedente dell'attività è ancora in esecuzione

journal.cli.think.activity.work.unreadable = non è possibile leggere il lavoro dell'attività { $path }: { $error }

journal.cli.think.activity.work.version = versione non supportata del lavoro dell'attività

journal.cli.think.argument.integer = argomento { $option }: valore int non valido

journal.cli.think.argument.unknown = argomenti non riconosciuti: { $argument }

journal.cli.think.argument.value = argomento { $option }: atteso un argomento

journal.cli.think.cancelled = ragionamento annullato

journal.cli.think.daily.blockers.more = { $names }, +{ $count } in più

journal.cli.think.daily.incomplete = { $units ->
    [one] { $blockers ->
        [one] { $phase } ({ $units } unità giornaliera non terminale; { $blockers } bloccante di segmento resta{ $details })
        [many] { $phase } ({ $units } unità giornaliera non terminale; { $blockers } di bloccanti di segmento restano{ $details })
       *[other] { $phase } ({ $units } unità giornaliera non terminale; { $blockers } bloccanti di segmento restano{ $details })
    }
    [many] { $blockers ->
        [one] { $phase } ({ $units } di unità giornaliere non terminali; { $blockers } bloccante di segmento resta{ $details })
        [many] { $phase } ({ $units } di unità giornaliere non terminali; { $blockers } di bloccanti di segmento restano{ $details })
       *[other] { $phase } ({ $units } di unità giornaliere non terminali; { $blockers } bloccanti di segmento restano{ $details })
    }
   *[other] { $blockers ->
        [one] { $phase } ({ $units } unità giornaliere non terminali; { $blockers } bloccante di segmento resta{ $details })
        [many] { $phase } ({ $units } unità giornaliere non terminali; { $blockers } di bloccanti di segmento restano{ $details })
       *[other] { $phase } ({ $units } unità giornaliere non terminali; { $blockers } bloccanti di segmento restano{ $details })
    }
    }

journal.cli.think.daily.incomplete.record = { $phase } ({ $units } unità giornaliere non terminali; { $blockers } bloccanti di segmento restano{ $details })

journal.cli.think.daily.inputs.changed = l'input grezzo è cambiato durante l'elaborazione dell'intera giornata

journal.cli.think.daily.location.packet = il riutilizzo della posizione giornaliera non ha un pacchetto memorizzato

journal.cli.think.daily.location.read = non è stato possibile leggere la posizione giornaliera

journal.cli.think.daily.snapshot.changed = la prova giornaliera è cambiata prima della preparazione

journal.cli.think.daily.snapshot.changed.after = la prova giornaliera o il contratto sono cambiati durante la preparazione

journal.cli.think.daily.snapshot.freeze = preparazione dell'input giornaliero: { $error }

journal.cli.think.daily.snapshot.parent = la radice del talento non ha una directory superiore

journal.cli.think.daily.snapshot.removed = talento giornaliero rimosso durante la preparazione

journal.cli.think.error = { $command }: { $message }

journal.cli.think.error.with.log = { $primary }; { $log }

journal.cli.think.health.day.invalid = giorno non valido { $day }; atteso { $date_format }

journal.cli.think.health.directory = non è possibile leggere la directory { $path }: { $detail }

journal.cli.think.health.marker = errore del marcatore di stato: { $detail }

journal.cli.think.health.marker.atomic = non è stato possibile pubblicare il marcatore di stato: { $error }

journal.cli.think.health.marker.fingerprint = non è stato possibile leggere l'impronta del marcatore di stato: { $detail }

journal.cli.think.health.marker.io = l'I/O del marcatore di stato non è riuscito in { $path }: { $error }

journal.cli.think.health.marker.lock = blocco del marcatore di stato non riuscito: { $error }

journal.cli.think.health.marker.malformed = il marcatore di stato è malformato: { $path }

journal.cli.think.health.marker.stream = il marcatore di stato del flusso è malformato

journal.cli.think.health.metadata = non è possibile leggere i metadati del file { $path }: { $detail }

journal.cli.think.health.progress.unreadable = l'avanzamento del segmento contiene registri illeggibili

journal.cli.think.health.read = errore di I/O del diario: { $detail }

journal.cli.think.health.source = errore della fonte del registro di stato: { $detail }

journal.cli.think.help = uso: { $syntax }

journal.cli.think.host.sense.missing = l'esecutore Sense ospitato non è disponibile

journal.cli.think.intent.gone = la richiesta distruttiva non è più in attesa

journal.cli.think.log.activity.join = { $command }: { $day }/{ $segment } in ritardo si è unito alla sua attività aperta

journal.cli.think.log.activity.no.advance = { $command }: { $day }/{ $segment } non fa avanzare lo stato in corso dell'attività

journal.cli.think.log.activity.replay = { $command }: { $day }/{ $segment } in ritardo viene rieseguito con il giorno del suo flusso

journal.cli.think.log.activity.turn = avanzamento dello stato dell'attività di { $stream } senza il suo turno: { $error }

journal.cli.think.log.budget = l'esito del ragionamento supera il budget di registri del registro operativo

journal.cli.think.log.cadence.read = Non è stato possibile caricare lo stato della cadenza: { $error }

journal.cli.think.log.cadence.set.aside = Non è stato possibile caricare lo stato della cadenza: artefatto danneggiato messo da parte

journal.cli.think.log.daily.search = non è stato possibile esaminare il lavoro di ricerca prima del ragionamento giornaliero: { $error }

journal.cli.think.log.day.format = il giorno deve essere { $date_format }

journal.cli.think.log.day.local = giorno locale non valido

journal.cli.think.log.display.metadata = non è stato possibile salvare i metadati di visualizzazione per { $path }: { $error }

journal.cli.think.log.failed = registro dell'esecuzione del ragionamento { $path } non riuscito: { $detail }

journal.cli.think.log.failure.lock = blocco compromesso dell'errore del registro dell'esecuzione del ragionamento

journal.cli.think.log.held.release = non è stato possibile rilasciare il trattenimento di { $name } per { $segment }: { $error }

journal.cli.think.log.lock = blocco compromesso del registro dell'esecuzione del ragionamento

journal.cli.think.log.name.answers = risposte sui nomi: { $answers } cambiate, { $judged } attività giudicate di nuovo, { $changed } cambiati, { $waiting } in attesa

journal.cli.think.log.provider = { $left ->
    [one] il fornitore di ragionamento non è disponibile; { $left } segmento attende la prossima esecuzione
    [many] il fornitore di ragionamento non è disponibile; { $left } di segmenti attendono la prossima esecuzione
   *[other] il fornitore di ragionamento non è disponibile; { $left } segmenti attendono la prossima esecuzione
    }

journal.cli.think.log.provider.record = il fornitore di ragionamento non è disponibile; { $left } segmenti attendono la prossima esecuzione

journal.cli.think.log.reconciliation = riconciliazione del nuovo tentativo non riuscita: { $error }

journal.cli.think.log.segment.turn = ragionamento di { $day }/{ $segment } senza il suo turno: { $error }

journal.cli.think.markdown.entities = Entità Sense

journal.cli.think.markdown.entity = { $kind } — { $name } (ruolo={ $role }, fonte={ $source }) — { $context }

journal.cli.think.markdown.object.both = { $frames ->
    [one] { $photos ->
        [one] { $class }: in { $frames } fotogramma e { $photos } foto
        [many] { $class }: in { $frames } fotogramma e { $photos } di foto
       *[other] { $class }: in { $frames } fotogramma e { $photos } foto
    }
    [many] { $photos ->
        [one] { $class }: in { $frames } di fotogrammi e { $photos } foto
        [many] { $class }: in { $frames } di fotogrammi e { $photos } di foto
       *[other] { $class }: in { $frames } di fotogrammi e { $photos } foto
    }
   *[other] { $photos ->
        [one] { $class }: in { $frames } fotogrammi e { $photos } foto
        [many] { $class }: in { $frames } fotogrammi e { $photos } di foto
       *[other] { $class }: in { $frames } fotogrammi e { $photos } foto
    }
    }

journal.cli.think.markdown.object.both.maximum = { $frames ->
    [one] { $photos ->
        [one] { $class }: in { $frames } fotogramma e { $photos } foto, fino a { $most } alla volta
        [many] { $class }: in { $frames } fotogramma e { $photos } di foto, fino a { $most } alla volta
       *[other] { $class }: in { $frames } fotogramma e { $photos } foto, fino a { $most } alla volta
    }
    [many] { $photos ->
        [one] { $class }: in { $frames } di fotogrammi e { $photos } foto, fino a { $most } alla volta
        [many] { $class }: in { $frames } di fotogrammi e { $photos } di foto, fino a { $most } alla volta
       *[other] { $class }: in { $frames } di fotogrammi e { $photos } foto, fino a { $most } alla volta
    }
   *[other] { $photos ->
        [one] { $class }: in { $frames } fotogrammi e { $photos } foto, fino a { $most } alla volta
        [many] { $class }: in { $frames } fotogrammi e { $photos } di foto, fino a { $most } alla volta
       *[other] { $class }: in { $frames } fotogrammi e { $photos } foto, fino a { $most } alla volta
    }
    }

journal.cli.think.markdown.object.frames = { $frames ->
    [one] { $class }: in { $frames } fotogramma
    [many] { $class }: in { $frames } di fotogrammi
   *[other] { $class }: in { $frames } fotogrammi
    }

journal.cli.think.markdown.object.frames.maximum = { $frames ->
    [one] { $class }: in { $frames } fotogramma, fino a { $most } alla volta
    [many] { $class }: in { $frames } di fotogrammi, fino a { $most } alla volta
   *[other] { $class }: in { $frames } fotogrammi, fino a { $most } alla volta
    }

journal.cli.think.markdown.object.photos = { $photos ->
    [one] { $class }: in { $photos } foto
    [many] { $class }: in { $photos } di foto
   *[other] { $class }: in { $photos } foto
    }

journal.cli.think.markdown.object.photos.maximum = { $photos ->
    [one] { $class }: in { $photos } foto, fino a { $most } alla volta
    [many] { $class }: in { $photos } di foto, fino a { $most } alla volta
   *[other] { $class }: in { $photos } foto, fino a { $most } alla volta
    }

journal.cli.think.markdown.objects = Oggetti

journal.cli.think.markdown.partial.counts = Omesso: { $details }

journal.cli.think.markdown.partial.input = Omesso: Una parte dell'input non ci stava. Questo risultato è stato scritto da una parte dell'input.

journal.cli.think.markdown.partial.story = Questo racconto, e tutto ciò che è stato salvato con esso, è stato scritto da una parte dell'attività, non da tutta.

journal.cli.think.markdown.quotes = Citazioni

journal.cli.think.markdown.sense.entities = ### Entità

journal.cli.think.markdown.sense.facets = ### Facette

journal.cli.think.markdown.sense.record = ## Sense: { $kind }

journal.cli.think.markdown.sense.speakers = **Parlanti:** { $names }

journal.cli.think.markdown.speaker = Parlante { $number }

journal.cli.think.markdown.terminal.clock = { $clock } · terminale

journal.cli.think.markdown.terminal.notice = testo mostrato nel tuo terminale. questo dice ciò che era visibile, non ciò che hai letto, digitato o fatto. mascherare password e altri segreti è al meglio delle possibilità; le tue regole di oscuramento non si applicano.

journal.cli.think.markdown.topics = Temi

journal.cli.think.outcome.completed = { $command }: { $count } completati

journal.cli.think.outcome.failed = { $command }: { $count } non riusciti

journal.cli.think.phase.daily.prerequisites = il lavoro giornaliero è in attesa dei prerequisiti dei suoi segmenti

journal.cli.think.phase.exit = uscita { $code }

journal.cli.think.phase.segment.prerequisites = i prerequisiti del segmento sono cambiati

journal.cli.think.plan.activity.count = segmenti: { $segments }

journal.cli.think.plan.activity.empty = Nessun agente corrisponde al tipo di attività '{ $kind }'

journal.cli.think.plan.activity.heading = { $operation ->
    [refresh] Giorno { $day } { $activity_flag } { $activity } { $facet_flag } { $facet } (aggiornamento)
   *[ordinary] Giorno { $day } { $activity_flag } { $activity } { $facet_flag } { $facet }
    }

journal.cli.think.plan.activity.missing = Attività non trovata: { $activity } nella facetta '{ $facet }' il { $day }

journal.cli.think.plan.activity.type = tipo:     { $kind }

journal.cli.think.plan.cadence = Giorno { $day } — agenti di cadenza

journal.cli.think.plan.cadence.fire = { $segments ->
    [one] { $activities ->
        [one] avvia  { $name } — finestra: { $segments } segmento, { $activities } attività
        [many] avvia  { $name } — finestra: { $segments } segmento, { $activities } di attività
       *[other] avvia  { $name } — finestra: { $segments } segmento, { $activities } attività
    }
    [many] { $activities ->
        [one] avvia  { $name } — finestra: { $segments } di segmenti, { $activities } attività
        [many] avvia  { $name } — finestra: { $segments } di segmenti, { $activities } di attività
       *[other] avvia  { $name } — finestra: { $segments } di segmenti, { $activities } attività
    }
   *[other] { $activities ->
        [one] avvia  { $name } — finestra: { $segments } segmenti, { $activities } attività
        [many] avvia  { $name } — finestra: { $segments } segmenti, { $activities } di attività
       *[other] avvia  { $name } — finestra: { $segments } segmenti, { $activities } attività
    }
    }

journal.cli.think.plan.cadence.fire.record = avvia  { $name } — finestra: { $segments } segmenti, { $activities } attività

journal.cli.think.plan.cadence.idle = no-op { $name } — nessun nuovo lavoro dall'ultima esecuzione della cadenza

journal.cli.think.plan.cadence.skip = salta  { $name } — intervallo non trascorso ({ $seconds }s < { $minutes }m)

journal.cli.think.plan.day = Giorno { $day }

journal.cli.think.plan.day.refresh = Giorno { $day } (aggiornamento)

journal.cli.think.plan.day.segment = Giorno { $day } segmento { $segment }

journal.cli.think.plan.day.segment.refresh = Giorno { $day } segmento { $segment } (aggiornamento)

journal.cli.think.plan.empty.cadence = Nessun prompt per la pianificazione: { $cadence }

journal.cli.think.plan.empty.weekly = Nessun prompt per la pianificazione: { $weekly }

journal.cli.think.plan.flush = Giorno { $day } { $flush_flag } segmento { $segment }

journal.cli.think.plan.flush.empty = Nessun agente idoneo allo svuotamento

journal.cli.think.plan.flush.talent = { $kind ->
    [generate] gen  { $name }
   *[agent] agent  { $name }
    }

journal.cli.think.plan.idle = segmenti inattivi: scrivi solo stub (a meno di { $refresh_flag })

journal.cli.think.plan.orchestrator = Sense orchestratore (lineare):

journal.cli.think.plan.post.indexer = Fase successiva: { $command }

journal.cli.think.plan.post.stats = Fase successiva: { $command }

journal.cli.think.plan.pre = Fase precedente:  { $command } { $day } { $jobs_flag } { $jobs }

journal.cli.think.plan.priority = Priorità { $priority }:

journal.cli.think.plan.reprocess = { $count ->
    [one] Giorno { $day } — rielabora { $count } segmento
    [many] Giorno { $day } — rielabora { $count } di segmenti
   *[other] Giorno { $day } — rielabora { $count } segmenti
    }

journal.cli.think.plan.reprocess.record = Giorno { $day } — rielabora { $count } segmenti

journal.cli.think.plan.schedule.empty = Nessun prompt per la pianificazione: { $schedule }

journal.cli.think.plan.segment.row = [{ $index }/{ $total }] { $segment } ({ $start }-{ $end }) flusso={ $stream }

journal.cli.think.plan.skip = salta { $name } — nessuna attività: { $facets }

journal.cli.think.plan.state = macchina a stati dell'attività: aggiornamenti per segmento

journal.cli.think.plan.step.entities = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (esiste) — sempre per i non inattivi
    [new] { $step }. { $name } ({ $kind }/{ $format }) (nuovo) — sempre per i non inattivi
   *[plain] { $step }. { $name } ({ $kind }/{ $format }) — sempre per i non inattivi
    }

journal.cli.think.plan.step.possible = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (esiste) — se una trascrizione del parlato (non blocca mai il segmento)
    [new] { $step }. { $name } ({ $kind }/{ $format }) (nuovo) — se una trascrizione del parlato (non blocca mai il segmento)
   *[plain] { $step }. { $name } ({ $kind }/{ $format }) — se una trascrizione del parlato (non blocca mai il segmento)
    }

journal.cli.think.plan.step.screen = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (esiste) — se { $recommend_screen_record }
    [new] { $step }. { $name } ({ $kind }/{ $format }) (nuovo) — se { $recommend_screen_record }
   *[plain] { $step }. { $name } ({ $kind }/{ $format }) — se { $recommend_screen_record }
    }

journal.cli.think.plan.step.sense = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (esiste) — obbligatorio
    [new] { $step }. { $name } ({ $kind }/{ $format }) (nuovo) — obbligatorio
   *[plain] { $step }. { $name } ({ $kind }/{ $format }) — obbligatorio
    }

journal.cli.think.plan.step.speaker.attribution = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (esiste) — se embeddings audio (lo strato del modello solo se { $recommend_speaker_attribution })
    [new] { $step }. { $name } ({ $kind }/{ $format }) (nuovo) — se embeddings audio (lo strato del modello solo se { $recommend_speaker_attribution })
   *[plain] { $step }. { $name } ({ $kind }/{ $format }) — se embeddings audio (lo strato del modello solo se { $recommend_speaker_attribution })
    }

journal.cli.think.plan.table.facet = { $row ->
    [generator_existing] gen  { $name }/{ $facet } (esiste)
    [generator_new] gen  { $name }/{ $facet } (nuovo)
    [generator_plain] gen  { $name }/{ $facet }
    [agent_existing] agent  { $name }/{ $facet } (esiste)
    [agent_new] agent  { $name }/{ $facet } (nuovo)
   *[agent_plain] agent  { $name }/{ $facet }
    }

journal.cli.think.plan.table.talent = { $row ->
    [generator_existing] gen  { $name } (esiste)
    [generator_new] gen  { $name } (nuovo)
    [generator_plain] gen  { $name }
    [agent_existing] agent  { $name } (esiste)
    [agent_new] agent  { $name } (nuovo)
   *[agent_plain] agent  { $name }
    }

journal.cli.think.plan.total = { $total ->
    [one] Totale: { $total } agente
    [many] Totale: { $total } di agenti
   *[other] Totale: { $total } agenti
    }

journal.cli.think.plan.total.record = Totale: { $total } agenti

journal.cli.think.plan.weekly = Giorno { $day } — agenti settimanali

journal.cli.think.preflight.disk = Il tuo diario ha esaurito lo spazio sul suo disco ({ $detail }). Riparazione: libera spazio su quel disco, poi avvia di nuovo il tuo diario.

journal.cli.think.preflight.held = { $reason }: { $role_key }{ $role } { $pid_key }{ $pid } { $time_key }{ $started } { $generation_key }{ $generation } { $age_key }{ $age }; usa il percorso supervisionato o arresta quel processo in modo pulito

journal.cli.think.preflight.held.unavailable = { $reason }: { $owner }; usa il percorso supervisionato o arresta il processo attuale in modo pulito

journal.cli.think.priority.environment = priorità del lavoro non valida: { $error }

journal.cli.think.priority.syntax = priorità del lavoro non valida; attesi la fascia 1–6 e un orario della fonte

journal.cli.think.repair.segment.missing = segmento di riparazione mancante

journal.cli.think.reprocess.record = non è stato possibile registrare l'esito della rielaborazione: { $error }

journal.cli.think.retry.activity.changed = permesso del nuovo tentativo dell'attività cambiato

journal.cli.think.retry.activity.released = l'attività non è più messa da parte

journal.cli.think.retry.daily.changed = permesso del nuovo tentativo giornaliero cambiato

journal.cli.think.retry.daily.missing = permesso giornaliero mancante

journal.cli.think.retry.daily.released = l'unità giornaliera non è più trattenuta

journal.cli.think.retry.daily.terminal.missing = registro finale giornaliero mancante

journal.cli.think.retry.daily.unresolved = il nuovo tentativo giornaliero non è ancora risolto

journal.cli.think.retry.daily.unused = lo scrittore giornaliero non ha consumato il permesso del nuovo tentativo

journal.cli.think.retry.day.mismatch = il giorno del nuovo tentativo non corrisponde

journal.cli.think.retry.evidence.missing = manca la prova del nuovo tentativo

journal.cli.think.retry.permission.changed = permesso del nuovo tentativo cambiato

journal.cli.think.retry.raw.coordinate.invalid = coordinata nativa del nuovo tentativo non valida

journal.cli.think.retry.raw.coordinate.utf8 = coordinata di nuovo tentativo non-UTF8

journal.cli.think.retry.raw.fingerprint.invalid = l'impronta dell'input dell'errore nativo è malformata

journal.cli.think.retry.raw.handler.missing = l'errore nativo non ha un gestore

journal.cli.think.retry.raw.header.incomplete = l'intestazione dell'elaborazione nativa è incompleta

journal.cli.think.retry.raw.header.kind = l'intestazione dell'elaborazione nativa non è un oggetto

journal.cli.think.retry.raw.kind = non è un'analisi nativa

journal.cli.think.retry.raw.parent.missing = l'output nativo non ha una directory superiore

journal.cli.think.retry.raw.permission.unused = lo scrittore nativo non ha consumato il permesso del nuovo tentativo

journal.cli.think.retry.raw.receipt.changed = ricevuta finale nativa cambiata

journal.cli.think.retry.raw.receipt.missing = manca la ricevuta finale nativa

journal.cli.think.retry.raw.receipt.unpublished = lo scrittore nativo non ha pubblicato la ricevuta finale di questo nuovo tentativo

journal.cli.think.retry.raw.receipt.unreadable = la ricevuta nativa del terminale è illeggibile

journal.cli.think.retry.raw.record.malformed = il registro di elaborazione nativo è malformato

journal.cli.think.retry.raw.released = l'output nativo non è più trattenuto

journal.cli.think.retry.raw.schema.unknown = lo schema di elaborazione nativo non è riconosciuto

journal.cli.think.retry.raw.source.invalid = il fallimento nativo non ha un nome di file di origine sicuro

journal.cli.think.retry.raw.source.mismatch = l'origine del fallimento nativo non corrisponde al suo output

journal.cli.think.retry.segment.changed = il permesso di nuovo tentativo del segmento è cambiato

journal.cli.think.retry.talent.disabled = il talento trattenuto non è più attivo

journal.cli.think.retry.talent.dispatch = non è stato possibile inviare il talento trattenuto

journal.cli.think.retry.talent.segment.missing = il segmento non esiste più

journal.cli.think.retry.talent.skipped = non è stato possibile riprovare il talento trattenuto

journal.cli.think.routing.log.malformed = non è possibile leggere il ripristino dell'instradamento dai registri di stato malformati

journal.cli.think.routing.source.invalid = la fonte di instradamento conservata non è valida

journal.cli.think.segment.ambiguous = { $count ->
    [one] il segmento ambiguo { $day }/{ $segment }: { $count } flusso condivide questo nome
    [many] il segmento ambiguo { $day }/{ $segment }: { $count } di flussi condividono questo nome
   *[other] il segmento ambiguo { $day }/{ $segment }: { $count } flussi condividono questo nome
}

journal.cli.think.sense.complete = L'elaborazione batch è completata

journal.cli.think.sense.config.concurrency = sense: { $handler }.{ $setting } non valido; si usa 1 come predefinito

journal.cli.think.sense.config.read = sense: la configurazione del diario non è disponibile: { $error }

journal.cli.think.sense.config.runtime = sense: { $handler }.{ $setting } non valido; si usa { $seconds }s come predefinito

journal.cli.think.sense.conflict = { $sense_batch_flag } è incompatibile con { $options }

journal.cli.think.sense.day = { $sense_batch_flag } richiede { $day_flag }

journal.cli.think.sense.day.missing = La directory del giorno non è stata trovata: { $path }

journal.cli.think.sense.deferred = { $quantity ->
    [one] L'elaborazione batch è differita: { $deferred } di { $ran } gestore in attesa; i file non terminati verranno riprovati
    [many] L'elaborazione batch è differita: { $deferred } di { $ran } di gestori in attesa; i file non terminati verranno riprovati
   *[other] L'elaborazione batch è differita: { $deferred } di { $ran } gestori in attesa; i file non terminati verranno riprovati
}

journal.cli.think.sense.dry.empty = Nessun file non elaborato trovato

journal.cli.think.sense.dry.would.process = { $quantity ->
    [one] verrebbe elaborato { $count } file ({ $breakdown }):
    [many] verrebbero elaborati { $count } di file ({ $breakdown }):
   *[other] verrebbero elaborati { $count } file ({ $breakdown }):
}

journal.cli.think.sense.error.delete = non è stato possibile eliminare { $path }: { $error }

journal.cli.think.sense.error.deleted.dirty = la rielaborazione ha rimosso { $path }, ma non è stato possibile marcare in modo durevole il giorno { $day } come sporco: { $detail }

journal.cli.think.sense.error.failed = { $failed } non riusciti di { $ran } eseguiti

journal.cli.think.sense.error.held = il lease del batch di sense per il giorno { $day } è trattenuto da un altro processo

journal.cli.think.sense.error.join = Il worker di Sense non si è fermato in modo pulito

journal.cli.think.sense.error.lease = errore del lease del batch di sense: { $error }

journal.cli.think.sense.error.runtime = l'ambiente di esecuzione di callosum del batch non è disponibile

journal.cli.think.sense.error.scan = non è stato possibile scansionare { $path }: { $error }

journal.cli.think.sense.error.timeout = il batch di sense è scaduto dopo { $duration }

journal.cli.think.sense.files = { $quantity ->
    [one] Trovato { $count } file non elaborato da elaborare ({ $extensions })
    [many] Trovati { $count } di file non elaborati da elaborare ({ $extensions })
   *[other] Trovati { $count } file non elaborati da elaborare ({ $extensions })
}

journal.cli.think.sense.interrupted.write = non è stato possibile scrivere l'evento { $event } in { $path }: { $error }

journal.cli.think.sense.lease.skip = Il lease del batch di Sense per il giorno { $day } è trattenuto; si salta l'attraversamento del batch

journal.cli.think.sense.nothing = Nessun file non elaborato trovato in { $path }

journal.cli.think.sense.priority.encoding = { $variable } contiene UTF-8 non valido

journal.cli.think.sense.processing.day = { $quantity ->
    [one] L'elaborazione dei file del giorno { $day } con { $jobs } lavoro concorrente
    [many] L'elaborazione dei file del giorno { $day } con { $jobs } di lavori concorrenti
   *[other] L'elaborazione dei file del giorno { $day } con { $jobs } lavori concorrenti
}

journal.cli.think.sense.processing.segment = { $quantity ->
    [one] L'elaborazione dei file del giorno { $day } (segmento: { $segment }) con { $jobs } lavoro concorrente
    [many] L'elaborazione dei file del giorno { $day } (segmento: { $segment }) con { $jobs } di lavori concorrenti
   *[other] L'elaborazione dei file del giorno { $day } (segmento: { $segment }) con { $jobs } lavori concorrenti
}

journal.cli.think.sense.refused = il batch di sense rifiutato: { $reason }

journal.cli.think.sense.reprocess.deleted.count = { $quantity ->
    [one] eliminato { $count } file di output
    [many] eliminati { $count } di file di output
   *[other] eliminati { $count } file di output
}

journal.cli.think.sense.reprocess.deleted.file = eliminato: { $path }

journal.cli.think.sense.reprocess.dry.empty = nessun file da eliminare

journal.cli.think.sense.reprocess.dry.would.delete = { $quantity ->
    [one] verrebbe eliminato { $count } file di output:
    [many] verrebbero eliminati { $count } di file di output:
   *[other] verrebbero eliminati { $count } file di output:
}

journal.cli.think.sense.reprocess.kept.before.voice.only = { $quantity ->
    [one] conservata { $count } trascrizione creata prima che "lascia fuori le altre voci" fosse attivo: disattivalo per rielaborare quella trascrizione
    [many] conservate { $count } di trascrizioni create prima che "lascia fuori le altre voci" fosse attivo: disattivalo per rielaborarle
   *[other] conservate { $count } trascrizioni create prima che "lascia fuori le altre voci" fosse attivo: disattivalo per rielaborarle
}

journal.cli.think.sense.reprocess.kept.voice.only = { $quantity ->
    [one] conservato { $count } file di output creato con "lascia fuori le altre voci" attivo: non viene rielaborato
    [many] conservati { $count } di file di output creati con "lascia fuori le altre voci" attivo: non vengono rielaborati
   *[other] conservati { $count } file di output creati con "lascia fuori le altre voci" attivo: non vengono rielaborati
}

journal.cli.think.sense.retry.changed = gli input del nuovo tentativo nativo sono cambiati

journal.cli.think.sense.retry.handler = il gestore sconosciuto del nuovo tentativo nativo

journal.cli.think.sense.retry.kind = il nuovo tentativo non è un'analisi nativa

journal.cli.think.sense.retry.mismatch = il gestore del fallimento nativo non corrisponde alla sua origine

journal.cli.think.sense.worker.authority = l'autorità di avvio di { $handler } non era gestita

journal.cli.think.sense.worker.deferred = differito (uscita { $code })

journal.cli.think.sense.worker.detail = sense: day={ $day } stream={ $stream } segment={ $segment } file={ $file } error={ $error }

journal.cli.think.sense.worker.exit = { $handler } uscita { $code }

journal.cli.think.sense.worker.failed = non riuscito (uscita { $code })

journal.cli.think.sense.worker.launch = { $handler } spawn non riuscito

journal.cli.think.sense.worker.launch.failed = sense: avvio di { $handler } non riuscito: { $error }

journal.cli.think.sense.worker.log.tail = sense: la diagnostica del gestore log={ $log } tail={ $tail }

journal.cli.think.sense.worker.log.unreadable = sense: la diagnostica del gestore log={ $log } illeggibile: { $error }

journal.cli.think.sense.worker.output.incomplete = { $handler } è uscito con 0, con output non terminato (output dell'analisi incompleto): { $path }

journal.cli.think.sense.worker.output.nonregular = { $handler } è uscito con 0 senza un output regolare: { $path }

journal.cli.think.sense.worker.output.unfinished = { $handler } è uscito con 0, con output non terminato ({ $reason }): { $path }

journal.cli.think.sense.worker.output.unreadable = { $handler } è uscito con 0 senza output leggibile { $path }: { $error }

journal.cli.think.sense.worker.pool = { $handler } pool non disponibile

journal.cli.think.sense.worker.program.executable = { $handler } dispatcher current_exe non riuscito: { $error }

journal.cli.think.sense.worker.program.missing = { $handler } dispatcher mancante: { $path }

journal.cli.think.sense.worker.program.nonexecutable = { $handler } dispatcher non eseguibile: { $path }

journal.cli.think.sense.worker.program.nonregular = { $handler } dispatcher non è un file: { $path }

journal.cli.think.sense.worker.status = sense: handler={ $handler } file={ $file } outcome={ $outcome } log={ $log }

journal.cli.think.sense.worker.survivors = sense: l'albero dei processi non è stato raccolto handler={ $handler } reason={ $reason } survivors={ $count }

journal.cli.think.sense.worker.termination = sense: la terminazione gestita non è riuscita handler={ $handler }: { $error }

journal.cli.think.sense.worker.timeout = { $handler } watchdog_timeout dopo { $seconds }s

journal.cli.think.sense.worker.watchdog = watchdog o attesa del processo non riuscito

journal.cli.think.settle.save = lo stato di assestamento dell'attività non è salvato: { $error }

journal.cli.think.updated.capability = la capacità di messa in pari non è disponibile su questa piattaforma

journal.cli.think.updated.conflict = { $updated_flag } è incompatibile con { $options }

journal.cli.think.updated.io = l'I/O di messa in pari non è riuscito in { $path }: { $error }

journal.cli.think.updated.marker = errore del marcatore di messa in pari: { $error }

journal.cli.think.updated.path = errore del percorso del diario per la messa in pari: { $error }

journal.cli.think.updated.state = lo stato di messa in pari è malformato: { $detail }

journal.cli.think.usage.error = uso: { $syntax }{ $line_break }{ $command }: errore: { $message }

journal.cli.think.validation.activity.incompatible = { $activity_flag } è incompatibile con { $segment_flag }, { $segments_flag } e { $flush_flag }

journal.cli.think.validation.activity.requires.day = { $activity_flag } richiede { $day_flag }

journal.cli.think.validation.activity.requires.facet = { $activity_flag } richiede { $facet_flag }

journal.cli.think.validation.cadence.incompatible = { $cadence_flag } è incompatibile con { $segment_flag }, { $segments_flag }, { $activity_flag }, { $flush_flag } e { $weekly_flag }

journal.cli.think.validation.daily.only = { $daily_only_flag } richiede { $day_flag } e non può essere combinato con un'altra modalità di ragionamento

journal.cli.think.validation.facet.requires.activity = { $facet_flag } richiede { $activity_flag }

journal.cli.think.validation.flush.incompatible = { $flush_flag } è incompatibile con { $segments_flag } e { $refresh_flag }

journal.cli.think.validation.flush.requires.segment = { $flush_flag } richiede { $segment_flag }

journal.cli.think.validation.multi.worker.unlimited.jobs = { $jobs_flag } 0 è incompatibile con il multi-worker { $segments_flag }; imposta { $jobs_flag } su un limite positivo o { $segment_workers_flag } 1

journal.cli.think.validation.no.activity.prompts.with.activity = { $no_activity_prompts_flag } non può essere combinato con { $activity_flag }

journal.cli.think.validation.reactivate.incompatible.refresh = { $reactivate_flag } è incompatibile con { $refresh_flag }

journal.cli.think.validation.reactivate.requires.activity = { $reactivate_flag } richiede { $activity_flag }

journal.cli.think.validation.repair.segment = { $repair_segment_flag } richiede { $day_flag } e { $segment_flag } e non può essere combinato con un'altra modalità di ragionamento

journal.cli.think.validation.retry.unit = { $retry_unit_flag } richiede { $day_flag } e { $retry_cause_flag } senza un'altra modalità di ragionamento

journal.cli.think.validation.segment.workers.range = { $segment_workers_flag } deve essere compreso tra 1 e 32

journal.cli.think.validation.segments.incompatible = { $segments_flag } è incompatibile con { $segment_flag } e { $facet_flag }

journal.cli.think.validation.settle.incompatible = { $settle_flag } è incompatibile con { $segment_flag }, { $segments_flag }, { $activity_flag }, { $flush_flag }, { $refresh_flag }, { $weekly_flag } e { $cadence_flag }

journal.cli.think.validation.weekly.incompatible = { $weekly_flag } è incompatibile con { $segment_flag }, { $segments_flag }, { $activity_flag } e { $flush_flag }

journal.cli.think.validation.work.intent = { $work_intent_flag } richiede una richiesta giornaliera da zero

journal.cli.think.wait.cortex = l'attesa di cortex non è riuscita: { $error }

journal.cli.think.wait.failed = l'attesa non è riuscita: { $error }

journal.cli.transcribe.audio.directory = Il file audio deve essere in una directory di segmento ({ $pattern }), ma la directory superiore è: { $parent }

journal.cli.transcribe.audio.format = Il formato audio non è supportato: { $extension }. I formati supportati: { $formats }

journal.cli.transcribe.audio.missing = Il file audio non è stato trovato.{ $line_break }  Tentato assoluto:           { $absolute }{ $line_break }  Tentato relativo al diario: { $relative }

journal.cli.transcribe.backend.choice = l'argomento { $backend_flag }: scelta non valida: '{ $backend }' (scegli tra { $choices })

journal.cli.transcribe.backend.duplicate = l'argomento { $backend_flag }: può essere specificato solo una volta

journal.cli.transcribe.batch.day = { $count ->
    [one] { $verb }: non è stato possibile leggere { $count } giorno
    [many] { $verb }: non è stato possibile leggere { $count } di giorni
   *[other] { $verb }: non è stato possibile leggere { $count } giorni
}

journal.cli.transcribe.batch.day.record = { $verb }: non è stato possibile leggere { $count } giorni

journal.cli.transcribe.batch.failed = trascrizione: { $failed } non riuscite

journal.cli.transcribe.claim.input.cancelled = la verifica dell'input di elaborazione è annullata

journal.cli.transcribe.claim.input.changed.commit = l'input di elaborazione è cambiato prima della pubblicazione

journal.cli.transcribe.claim.input.changed.open = l'input di elaborazione è cambiato durante l'apertura

journal.cli.transcribe.claim.input.changed.read = l'input di elaborazione è cambiato durante la lettura

journal.cli.transcribe.claim.input.no.attempt = l'input non ha un tentativo di output

journal.cli.transcribe.claim.input.not.file = l'input di elaborazione non è un file regolare

journal.cli.transcribe.claim.lock.entry.unsafe = l'elemento del blocco persistente è { $kind } non sicuro: { $path }

journal.cli.transcribe.claim.lock.invalid = l'elemento del blocco persistente non è valido { $name }

journal.cli.transcribe.claim.lock.missing = manca la directory superiore del blocco persistente: { $path }

journal.cli.transcribe.claim.lock.mode = l'elemento del blocco persistente ha modo { $mode }, atteso 600: { $path }

journal.cli.transcribe.claim.lock.namespace = l'elemento del blocco persistente è cambiato durante l'acquisizione: { $path }

journal.cli.transcribe.claim.lock.parent.changed = la directory superiore del blocco persistente è cambiata durante l'acquisizione: { $path }

journal.cli.transcribe.claim.lock.parent.unsafe = la directory superiore del blocco persistente è { $kind } non sicuro: { $path }

journal.cli.transcribe.claim.parent.missing = manca la directory superiore

journal.cli.transcribe.claim.root.absent = la radice del diario non esiste

journal.cli.transcribe.claim.root.absolute = la radice del diario deve essere assoluta

journal.cli.transcribe.claim.root.ancestor = la radice canonica del diario ha un antenato non UTF-8

journal.cli.transcribe.claim.root.canonical.absolute = la radice canonica del diario non è assoluta

journal.cli.transcribe.claim.root.changed = l'origine del diario è cambiata durante l'acquisizione

journal.cli.transcribe.claim.root.component = la radice canonica del diario non ha un componente finale

journal.cli.transcribe.claim.root.directory = la radice del diario non è una directory

journal.cli.transcribe.claim.root.invalid = la radice del diario non è valida { $path }: { $detail }

journal.cli.transcribe.claim.root.io = { $operation }: { $reason }

journal.cli.transcribe.claim.root.unsupported = la radice del diario non è supportata { $path }: { $detail }

journal.cli.transcribe.claim.segment.not.covered = il segmento non è coperto da questi tentativi di output

journal.cli.transcribe.claim.target.name = manca il nome di destinazione del blocco

journal.cli.transcribe.config.backend = { $key } non valido nella configurazione del diario; scegli uno tra: { $backends }

journal.cli.transcribe.error.asset.absent = non è stato possibile trovare la risorsa del modello non vuota { $asset }; cercato { $paths }

journal.cli.transcribe.error.asset.digest = la risorsa del modello { $asset } in { $path } ha { $code_sha256 } { $actual }, atteso { $expected }

journal.cli.transcribe.error.asset.executable = non è stato possibile determinare l'eseguibile corrente: { $error }

journal.cli.transcribe.error.asset.override = { $code_solstone_transcribe_model_assets_dir }={ $directory } non fornisce la risorsa del modello non vuota { $asset }

journal.cli.transcribe.error.audio.budget = il budget di campioni decodificati { $limit } è superato per { $path }

journal.cli.transcribe.error.audio.corrupt = l'input audio è corrotto { $path }: { $detail }

journal.cli.transcribe.error.audio.empty = l'input audio è vuoto: { $path }

journal.cli.transcribe.error.audio.ffmpeg = { $code_ffmpeg } errore per { $path }: { $detail }

journal.cli.transcribe.error.audio.flush = { $iterations ->
    [one] { $flushed ->
        [one] lo svuotamento del ricampionatore non ha raggiunto la convergenza per { $path } dopo { $iterations } iterazione ({ $flushed } campione svuotato; { $remaining } ancora in ritardo)
        [many] lo svuotamento del ricampionatore non ha raggiunto la convergenza per { $path } dopo { $iterations } iterazione ({ $flushed } di campioni svuotati; { $remaining } ancora in ritardo)
       *[other] lo svuotamento del ricampionatore non ha raggiunto la convergenza per { $path } dopo { $iterations } iterazione ({ $flushed } campioni svuotati; { $remaining } ancora in ritardo)
    }
    [many] { $flushed ->
        [one] lo svuotamento del ricampionatore non ha raggiunto la convergenza per { $path } dopo { $iterations } di iterazioni ({ $flushed } campione svuotato; { $remaining } ancora in ritardo)
        [many] lo svuotamento del ricampionatore non ha raggiunto la convergenza per { $path } dopo { $iterations } di iterazioni ({ $flushed } di campioni svuotati; { $remaining } ancora in ritardo)
       *[other] lo svuotamento del ricampionatore non ha raggiunto la convergenza per { $path } dopo { $iterations } di iterazioni ({ $flushed } campioni svuotati; { $remaining } ancora in ritardo)
    }
   *[other] { $flushed ->
        [one] lo svuotamento del ricampionatore non ha raggiunto la convergenza per { $path } dopo { $iterations } iterazioni ({ $flushed } campione svuotato; { $remaining } ancora in ritardo)
        [many] lo svuotamento del ricampionatore non ha raggiunto la convergenza per { $path } dopo { $iterations } iterazioni ({ $flushed } di campioni svuotati; { $remaining } ancora in ritardo)
       *[other] lo svuotamento del ricampionatore non ha raggiunto la convergenza per { $path } dopo { $iterations } iterazioni ({ $flushed } campioni svuotati; { $remaining } ancora in ritardo)
    }
}

journal.cli.transcribe.error.audio.io = { $code_i_o } errore per { $path }: { $error }

journal.cli.transcribe.error.audio.sample.rate = la frequenza di campionamento { $code_wav } non è valida: { $rate }

journal.cli.transcribe.error.audio.samples = nessun campione audio decodificato da { $path }

journal.cli.transcribe.error.audio.sidecar.create = non è possibile creare lo sidecar { $code_f32le } { $path }: { $error }

journal.cli.transcribe.error.audio.sidecar.sync = non è possibile sincronizzare lo sidecar { $code_f32le } { $path }: { $error }

journal.cli.transcribe.error.audio.sidecar.write = non è possibile scrivere lo sidecar { $code_f32le } { $path }: { $error }

journal.cli.transcribe.error.audio.stream = nessun flusso audio in { $path }

journal.cli.transcribe.error.audio.wav.size = { $samples ->
    [one] I dati WAV sono troppo grandi per un campo { $riff_u32 }: { $samples } campione
    [many] I dati WAV sono troppo grandi per un campo { $riff_u32 }: { $samples } di campioni
   *[other] I dati WAV sono troppo grandi per un campo { $riff_u32 }: { $samples } campioni
}

journal.cli.transcribe.error.backend.memory = nessun backend { $code_stt } utilizzabile (memoria disponibile: { $available }, soglia locale: { $floor })

journal.cli.transcribe.error.backend.platform = il backend { $code_stt } non è disponibile su questa piattaforma host: { $backend }

journal.cli.transcribe.error.cancelled = la trascrizione incorporata è annullata prima della pubblicazione

journal.cli.transcribe.error.claim = non è stato possibile acquisire la proprietà della trascrizione per { $path }: { $detail }

journal.cli.transcribe.error.confidential.confidential.audio.handling.is.disabled = la gestione dell'audio confidenziale è disattivata

journal.cli.transcribe.error.confidential.confidential.lane.blocks.stt.backend.backend.raw.audio = la corsia confidenziale blocca il backend { $code_stt } { $backend }; l'audio grezzo deve restare in locale

journal.cli.transcribe.error.confidential.failure = la trascrizione confidenziale { $reason }: { $detail }

journal.cli.transcribe.error.confidential.hosted.stt.returned.http.status = { $code_stt } ospitato ha restituito { $code_http } { $status }

journal.cli.transcribe.error.confidential.hosted.stt.returned.http.unauthorized = { $code_stt } ospitato ha restituito { $code_http } 401

journal.cli.transcribe.error.confidential.the.confidential.admission.probe.failed = la sonda di ammissione confidenziale non è riuscita

journal.cli.transcribe.error.confidential.the.confidential.attestation.channel.is.not.ready = il canale di attestazione confidenziale non è pronto

journal.cli.transcribe.error.confidential.the.confidential.endpoint.has.no.credential = l'endpoint confidenziale non ha una credenziale

journal.cli.transcribe.error.confidential.the.confidential.lane.has.no.confidential.byo.endpoint = la corsia confidenziale non ha un endpoint { $code_byo } confidenziale

journal.cli.transcribe.error.confidential.the.confidential.lane.is.no.longer.active = la corsia confidenziale non è più attiva

journal.cli.transcribe.error.confidential.word.a.word.entry.was.blank = un elemento di parola era vuoto

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.a.required.key = a un elemento di parola mancava una chiave obbligatoria

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.end = a un elemento di parola mancava `end`

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.start = a un elemento di parola mancava `start`

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.word = a un elemento di parola mancava `word`

journal.cli.transcribe.error.confidential.word.a.word.entry.was.not.an.object = un elemento di parola non era un oggetto

journal.cli.transcribe.error.confidential.word.contract = la risposta { $code_stt } ospitata ha violato il contratto verbose { $code_json }: { $detail }

journal.cli.transcribe.error.confidential.word.response.had.no.words.array = la risposta non aveva un array di parole

journal.cli.transcribe.error.confidential.word.response.had.text.but.no.word.timings = la risposta aveva testo ma nessuna temporizzazione delle parole

journal.cli.transcribe.error.confidential.word.response.was.not.an.object = la risposta non era un oggetto

journal.cli.transcribe.error.confidential.word.response.was.not.json = la risposta non era { $code_json }

journal.cli.transcribe.error.confidential.word.word.key.was.not.a.finite.number.found = la parola `{ $key }` non era un numero finito (trovato { $found })

journal.cli.transcribe.error.coreml.failure = { $code_parakeet } { $code_coreml } { $reason }: { $detail }

journal.cli.transcribe.error.coreml.helper.json.was.not.an.object = l'ausiliario { $code_json } non era un oggetto

journal.cli.transcribe.error.coreml.helper.response.has.invalid.key = la risposta dell'ausiliario ha { $key } non valido

journal.cli.transcribe.error.coreml.helper.response.missing.key = manca { $key } nella risposta dell'ausiliario

journal.cli.transcribe.error.coreml.helper.response.missing.token.timings = manca { $code_token_timings } nella risposta dell'ausiliario

journal.cli.transcribe.error.coreml.helper.returned.transcript.text.without.token.timings = l'ausiliario ha restituito il testo della trascrizione senza temporizzazioni dei token

journal.cli.transcribe.error.coreml.helper.token.timings.collapsed.to.no.words = le temporizzazioni dei token dell'ausiliario sono collassate a nessuna parola

journal.cli.transcribe.error.coreml.helper.version.json.was.not.an.object = la versione dell'ausiliario { $code_json } non era un oggetto

journal.cli.transcribe.error.coreml.helper.version.response.missing.field = manca { $field } nella risposta della versione dell'ausiliario

journal.cli.transcribe.error.coreml.missing.stderr.pipe = manca la pipe { $code_stderr }

journal.cli.transcribe.error.coreml.missing.stdout.pipe = manca la pipe { $code_stdout }

journal.cli.transcribe.error.coreml.name.reader.panicked = il lettore { $name } è andato in panic

journal.cli.transcribe.error.coreml.parakeet.coreml.model.is.not.installed.yet.path = il modello { $code_parakeet } { $code_coreml } non è ancora installato: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.exited.with.status = l'ausiliario { $code_parakeet } è uscito con { $status }

journal.cli.transcribe.error.coreml.parakeet.helper.is.missing.path = l'ausiliario { $code_parakeet } manca: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.is.not.a.file.path = l'ausiliario { $code_parakeet } non è un file: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.is.not.executable.path = l'ausiliario { $code_parakeet } non è eseguibile: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.timed.out = l'ausiliario { $code_parakeet } è scaduto

journal.cli.transcribe.error.coreml.parakeet.helper.timed.out.after.seconds.s = l'ausiliario { $code_parakeet } è scaduto dopo { $seconds }s

journal.cli.transcribe.error.coreml.publication.busy = il blocco di pubblicazione { $code_parakeet } { $code_coreml } è occupato

journal.cli.transcribe.error.coreml.token.timing.must.be.an.object = la temporizzazione del token deve essere un oggetto

journal.cli.transcribe.error.cpp.device.must.be.one.of.auto.cpu = il dispositivo deve essere uno di: { $code_auto_cpu }

journal.cli.transcribe.error.cpp.failure = { $code_parakeet_cpp } { $reason }: { $detail }

journal.cli.transcribe.error.cpp.http.status = { $code_http } { $status }

journal.cli.transcribe.error.cpp.parakeet.cpp.is.only.supported.on.linux.and = { $code_parakeet_cpp } è supportato solo su linux e windows

journal.cli.transcribe.error.cpp.parakeet.server.credentials.are.unavailable.in.this.process = le credenziali del server { $code_parakeet } non sono disponibili in questo processo.

journal.cli.transcribe.error.cpp.parakeet.server.is.not.ready.yet = il server { $code_parakeet } non è ancora pronto.

journal.cli.transcribe.error.cpp.response.has.text.but.no.word.timings = la risposta ha testo ma nessuna temporizzazione delle parole

journal.cli.transcribe.error.cpp.response.json.was.invalid.error = la risposta { $code_json } non era valida: { $error }

journal.cli.transcribe.error.cpp.response.json.was.not.an.object = la risposta { $code_json } non era un oggetto

journal.cli.transcribe.error.cpp.response.missing.top.level.words = manca { $code_words } di primo livello nella risposta

journal.cli.transcribe.error.cpp.tls.is.required = { $code_tls } è obbligatorio

journal.cli.transcribe.error.cpp.transcription.request.timed.out = la richiesta di trascrizione è scaduta

journal.cli.transcribe.error.cpp.word.timing.contains.invalid.numeric.value = la temporizzazione della parola contiene un valore numerico non valido

journal.cli.transcribe.error.cpp.word.timing.item.must.be.an.object = l'elemento della temporizzazione della parola deve essere un oggetto

journal.cli.transcribe.error.cpp.word.timing.missing.key.key = manca la chiave nella temporizzazione della parola: { $key }

journal.cli.transcribe.error.cpp.word.timing.text.was.blank = il testo della temporizzazione della parola era vuoto

journal.cli.transcribe.error.input.inspect = non è stato possibile ispezionare l'input { $path }: { $detail }

journal.cli.transcribe.error.orphan = non è stato possibile rimuovere l'orfano { $code_npz } { $path }: { $detail }

journal.cli.transcribe.error.payload.parent = la destinazione della trascrizione non ha una directory superiore

journal.cli.transcribe.error.process.admission = l'ammissione dell'avvio ospitato non è riuscita: { $reason }

journal.cli.transcribe.error.process.capability = la capacità dell'host non è disponibile: { $needed }

journal.cli.transcribe.error.process.command = non è possibile eseguire lo spawn di un comando vuoto

journal.cli.transcribe.error.process.confirm = la conferma dopo lo spawn non è riuscita per il pid { $pid }: { $error }

journal.cli.transcribe.error.process.explicit = questa autorità non è esplicitamente priva di proprietario

journal.cli.transcribe.error.process.identity = non è stato possibile acquisire l'identità legata alla nascita per il pid avviato { $pid }

journal.cli.transcribe.error.process.log = non è stato possibile preparare il registro operativo: { $error }

journal.cli.transcribe.error.process.output = l'output del processo figlio non è disponibile

journal.cli.transcribe.error.process.spawn = non è stato possibile eseguire lo spawn del processo figlio: { $error }

journal.cli.transcribe.error.process.terminate = non è stato possibile terminare il processo figlio: { $error }

journal.cli.transcribe.error.process.unowned.reason = il motivo di { $code_explicitlyunowned } deve essere non vuoto

journal.cli.transcribe.error.publish.output = { $code_output_jsonl_path } è obbligatorio

journal.cli.transcribe.error.publish.parent = l'output non ha una directory superiore

journal.cli.transcribe.error.publish.raw = { $code_header_raw } è obbligatorio

journal.cli.transcribe.error.publish.sibling = il grezzo di elaborazione deve essere un nome di file fratello

journal.cli.transcribe.error.speaker.analysis = l'analisi del parlante non è riuscita: { $stage }/{ $reason }

journal.cli.transcribe.error.statement.field = l'enunciato della trascrizione completa non ha { $field } valido

journal.cli.transcribe.error.terminal.payload = non è stato possibile preparare il payload del terminale: { $detail }

journal.cli.transcribe.error.terminal.payload.path = non è stato possibile preparare o rimuovere il payload del terminale { $path }: { $detail }

journal.cli.transcribe.error.terminal.request = non è stato possibile serializzare la richiesta del writer del terminale: { $detail }

journal.cli.transcribe.error.transcript.payload = non è stato possibile preparare il payload della trascrizione: { $detail }

journal.cli.transcribe.error.transcript.payload.path = non è stato possibile preparare o rimuovere il payload della trascrizione { $path }: { $detail }

journal.cli.transcribe.error.transcript.request = non è stato possibile serializzare la richiesta del writer della trascrizione: { $detail }

journal.cli.transcribe.error.vad.contract.exit = { $code_vad } errore di contratto dell'ausiliario (uscita { $code }): { $detail }

journal.cli.transcribe.error.vad.contract.exit.stderr = { $code_vad } errore di contratto dell'ausiliario (uscita { $code }): { $detail }: { $stderr }

journal.cli.transcribe.error.vad.contract.signal = { $code_vad } errore di contratto dell'ausiliario (nessun codice di uscita): { $detail }

journal.cli.transcribe.error.vad.contract.signal.stderr = { $code_vad } errore di contratto dell'ausiliario (nessun codice di uscita): { $detail }: { $stderr }

journal.cli.transcribe.error.vad.could.not.create.vad.input.directory.error = non è stato possibile creare la directory di input di { $code_vad }: { $error }

journal.cli.transcribe.error.vad.could.not.remove.vad.input.directory.error = non è stato possibile rimuovere la directory di input di { $code_vad }: { $error }

journal.cli.transcribe.error.vad.could.not.remove.vad.input.directory.input.remains.in = non è stato possibile rimuovere la directory di input di { $code_vad }: l'input è ancora in uso

journal.cli.transcribe.error.vad.could.not.send.vad.request.error = non è stato possibile inviare la richiesta di { $code_vad }: { $error }

journal.cli.transcribe.error.vad.could.not.serialize.vad.request.error = non è stato possibile serializzare la richiesta di { $code_vad }: { $error }

journal.cli.transcribe.error.vad.error.empty = { $code_vad } la risposta di errore era vuota

journal.cli.transcribe.error.vad.error.many = { $code_vad } la risposta di errore conteneva più di una riga

journal.cli.transcribe.error.vad.error.utf8 = { $code_vad } la risposta di errore non era { $code_utf_8 }: { $error }

journal.cli.transcribe.error.vad.helper = { $code_vad } ausiliario { $reason } (uscita { $code }): { $detail }

journal.cli.transcribe.error.vad.launch.failed = non è stato possibile avviare l'ausiliario { $code_vad } { $path }: { $detail }

journal.cli.transcribe.error.vad.response.empty = { $code_vad } la risposta era vuota

journal.cli.transcribe.error.vad.response.many = { $code_vad } la risposta conteneva più di una riga

journal.cli.transcribe.error.vad.response.utf8 = { $code_vad } la risposta non era { $code_utf_8 }: { $error }

journal.cli.transcribe.error.vad.vad.error.response.lacks.schema.reason.or.detail = { $code_vad } la risposta di errore non ha schema, motivo o dettaglio

journal.cli.transcribe.error.vad.vad.error.response.must.be.an.object = { $code_vad } la risposta di errore deve essere un oggetto

journal.cli.transcribe.error.vad.vad.error.response.violates.its.schema = { $code_vad } la risposta di errore viola il suo schema

journal.cli.transcribe.error.vad.vad.error.response.was.not.json.error = { $code_vad } la risposta di errore non era { $code_json }: { $error }

journal.cli.transcribe.error.vad.vad.helper.stdin.was.unavailable = { $code_vad } { $code_stdin } dell'ausiliario non era disponibile

journal.cli.transcribe.error.vad.vad.helper.terminated.without.an.exit.code = { $code_vad } l'ausiliario è terminato senza un codice di uscita

journal.cli.transcribe.error.vad.vad.response.has.an.unknown.schema = { $code_vad } la risposta ha uno schema sconosciuto

journal.cli.transcribe.error.vad.vad.response.has.no.boolean.has.speech = { $code_vad } la risposta non ha il booleano { $code_has_speech }

journal.cli.transcribe.error.vad.vad.response.has.no.non.empty.field = { $code_vad } la risposta non ha { $field } non vuoto

journal.cli.transcribe.error.vad.vad.response.has.no.non.negative.finite.field = { $code_vad } la risposta non ha { $field } finito non negativo

journal.cli.transcribe.error.vad.vad.response.has.no.speech.array = { $code_vad } la risposta non ha un array di parlato

journal.cli.transcribe.error.vad.vad.response.must.be.an.object = { $code_vad } la risposta deve essere un oggetto

journal.cli.transcribe.error.vad.vad.response.was.not.json.error = { $code_vad } la risposta non era { $code_json }: { $error }

journal.cli.transcribe.error.vad.vad.speech.entry.ends.before.it.starts = { $code_vad } l'elemento del parlato finisce prima di iniziare

journal.cli.transcribe.error.vad.vad.speech.entry.has.no.valid.field.sample.index = { $code_vad } l'elemento del parlato non ha un indice di campione { $field } valido

journal.cli.transcribe.error.vad.vad.speech.entry.must.be.an.object = { $code_vad } l'elemento del parlato deve essere un oggetto

journal.cli.transcribe.error.vad.wait.failed = non è stato possibile attendere l'ausiliario { $code_vad }: { $detail }

journal.cli.transcribe.error.vad.write = non è stato possibile scrivere l'input di { $code_vad }: { $detail }

journal.cli.transcribe.error.voice.wait = la trascrizione di sola voce è in attesa: { $detail }

journal.cli.transcribe.error.windows.admission.deadline.exceeded = l'ammissione dell'ausiliario delimitato non è terminata prima della scadenza

journal.cli.transcribe.error.windows.blocked = { $detail }; la pulizia precedente dell'ausiliario blocca l'ammissione: { $blockers }

journal.cli.transcribe.error.windows.cancelled = l'ausiliario delimitato è stato annullato

journal.cli.transcribe.error.windows.cleanup = { $detail }; la pulizia era incompleta al ritorno: { $cleanup }

journal.cli.transcribe.error.windows.cleanup.blocked = { $detail }; la pulizia era incompleta al ritorno: { $cleanup }; la pulizia precedente dell'ausiliario blocca l'ammissione: { $blockers }

journal.cli.transcribe.error.windows.cleanup.contended = l'ammissione della pulizia dell'ausiliario delimitato è contesa

journal.cli.transcribe.error.windows.cleanup.pending = la pulizia precedente dell'ausiliario delimitato blocca l'ammissione

journal.cli.transcribe.error.windows.current.directory.not.directory = la directory corrente dell'ausiliario delimitato non è una directory

journal.cli.transcribe.error.windows.current.directory.outside.package = la directory corrente dell'ausiliario delimitato è fuori dalla radice del suo pacchetto

journal.cli.transcribe.error.windows.current.directory.unavailable = non è stato possibile canonizzare la directory corrente dell'ausiliario delimitato

journal.cli.transcribe.error.windows.deadline.exceeded = l'ausiliario delimitato non è terminato prima della scadenza

journal.cli.transcribe.error.windows.executable.not.file = l'eseguibile dell'ausiliario delimitato non è un file regolare

journal.cli.transcribe.error.windows.executable.outside.package = l'eseguibile dell'ausiliario delimitato è fuori dalla radice del suo pacchetto

journal.cli.transcribe.error.windows.executable.unavailable = non è stato possibile canonizzare l'eseguibile dell'ausiliario delimitato

journal.cli.transcribe.error.windows.input.limit.exceeded = l'input dell'ausiliario delimitato supera il limite di byte dichiarato

journal.cli.transcribe.error.windows.input.write.failed = lo scrittore di input dell'ausiliario delimitato non è riuscito

journal.cli.transcribe.error.windows.io.worker.start.failed = non è stato possibile avviare il worker { $stream } { $code_i_o } dell'ausiliario delimitato

journal.cli.transcribe.error.windows.job.not.quiescent = il completamento dell'ausiliario delimitato non ha stabilito la quiescenza del Job

journal.cli.transcribe.error.windows.launch.failed = l'ausiliario delimitato non è riuscito prima che fosse disponibile un'identità propria del processo figlio

journal.cli.transcribe.error.windows.launch.finalization.failed = la finalizzazione dell'avvio nativo non è riuscita: { $detail }

journal.cli.transcribe.error.windows.missing.system.root = l'ambiente dell'ausiliario delimitato deve contenere { $code_systemroot } non vuoto

journal.cli.transcribe.error.windows.output.limit.exceeded = l'output { $stream } dell'ausiliario delimitato ha superato il limite di byte dichiarato

journal.cli.transcribe.error.windows.output.read.failed = il lettore di output { $stream } dell'ausiliario delimitato non è riuscito

journal.cli.transcribe.error.windows.package.root.unavailable = non è stato possibile canonizzare la radice del pacchetto dell'ausiliario delimitato

journal.cli.transcribe.error.windows.path.environment.refused = l'ambiente dell'ausiliario delimitato non può contenere { $code_path }

journal.cli.transcribe.error.windows.path.not.representable = il percorso dell'ausiliario delimitato non può essere rappresentato dal limite gestito dei comandi di Windows

journal.cli.transcribe.error.windows.process.observation.failed = l'osservazione del processo dell'ausiliario delimitato non è riuscita

journal.cli.transcribe.error.windows.zero.limit = il limite di byte { $stream } dell'ausiliario delimitato deve essere diverso da zero

journal.cli.transcribe.error.windows.zero.timeout = il timeout dell'ausiliario delimitato deve essere diverso da zero

journal.cli.transcribe.error.writer.base.time.us.of.day.is.outside.a.day = { $code_base_time_us_of_day } è fuori da un giorno

journal.cli.transcribe.error.writer.byte.count.does.not.match.shape = { $code_byte_count } non corrisponde alla forma

journal.cli.transcribe.error.writer.byte.count.is.too.large = { $code_byte_count } è troppo grande

journal.cli.transcribe.error.writer.cancelled = la pubblicazione della trascrizione è stata annullata prima dell'ammissione

journal.cli.transcribe.error.writer.destination = la destinazione esiste già: { $path }

journal.cli.transcribe.error.writer.dtype.must.be.float32.le = { $code_dtype } deve essere { $code_float32_le }

journal.cli.transcribe.error.writer.duration.and.noisy.must.be.provided.together = duration e noisy devono essere forniti insieme

journal.cli.transcribe.error.writer.durations.s.count.does.not.match.shape = il conteggio di { $code_durations_s } non corrisponde alla forma

journal.cli.transcribe.error.writer.durations.s.must.be.finite.f32.values = { $code_durations_s } deve avere valori { $code_f32 } finiti

journal.cli.transcribe.error.writer.encoder.must.be.nonempty = il codificatore deve essere non vuoto

journal.cli.transcribe.error.writer.field.must.be.a.boolean = { $field } deve essere un booleano

journal.cli.transcribe.error.writer.field.must.be.a.finite.number = { $field } deve essere un numero finito

journal.cli.transcribe.error.writer.field.must.be.a.string = { $field } deve essere una stringa

journal.cli.transcribe.error.writer.field.must.be.an.array = { $field } deve essere un array

journal.cli.transcribe.error.writer.field.must.be.an.integer = { $field } deve essere un intero

journal.cli.transcribe.error.writer.field.must.be.an.unsigned.integer = { $field } deve essere un intero senza segno

journal.cli.transcribe.error.writer.header.numbers.must.be.finite = i numeri dell'intestazione devono essere finiti

journal.cli.transcribe.error.writer.id.range = l'id deve essere in { $code_1_i32_max }

journal.cli.transcribe.error.writer.invalid.f32.payload.chunk = frammento di payload { $code_f32 } non valido

journal.cli.transcribe.error.writer.jsonl.path.and.npz.path.must.be.sibling.files = { $code_jsonl_path } e { $code_npz_path } devono essere file fratelli che condividono il nome base

journal.cli.transcribe.error.writer.missing.required.field.key = manca il campo obbligatorio { $key }

journal.cli.transcribe.error.writer.noisy.s.is.required.when.noisy.rms.is.present = { $code_noisy_s } è obbligatorio quando { $code_noisy_rms } è presente

journal.cli.transcribe.error.writer.npz.cancelled = annullato

journal.cli.transcribe.error.writer.npz.count = { $code_npz } ha un numero di membri inatteso

journal.cli.transcribe.error.writer.npz.members = i membri di { $code_npz } non corrispondono al contratto del sidecar della trascrizione

journal.cli.transcribe.error.writer.npz.missing = { $code_npz } membro { $name } è mancante

journal.cli.transcribe.error.writer.npz.parent = la destinazione npz non ha una directory superiore

journal.cli.transcribe.error.writer.npz.readback = { $code_npz } membro { $name } non corrisponde in rilettura

journal.cli.transcribe.error.writer.output.path.has.no.parent = il percorso di output non ha una directory superiore

journal.cli.transcribe.error.writer.output.paths.must.be.nonempty = i percorsi di output devono essere non vuoti

journal.cli.transcribe.error.writer.payload.finite = il payload dell'embedding non è un valore finito alla riga { $row }, colonna { $column }

journal.cli.transcribe.error.writer.payload.format.must.be.raw.f32le.row.major.v1 = { $code_payload_format } deve essere { $code_raw_f32le_row_major_v1 }

journal.cli.transcribe.error.writer.payload.size.does.not.match.byte.count = la dimensione del payload non corrisponde a { $code_byte_count }

journal.cli.transcribe.error.writer.prior.jsonl.snapshot.is.missing = manca l'istantanea jsonl precedente

journal.cli.transcribe.error.writer.prior.npz.snapshot.is.missing = manca l'istantanea npz precedente

journal.cli.transcribe.error.writer.prior.restore.refused = il ripristino precedente è stato rifiutato

journal.cli.transcribe.error.writer.private.stage.is.missing = manca la preparazione privata

journal.cli.transcribe.error.writer.private.transcript.copy.did.not.match.the.staged.file = la copia privata della trascrizione non corrispondeva al file preparato

journal.cli.transcribe.error.writer.private.transcript.jsonl.is.missing = manca il jsonl privato della trascrizione

journal.cli.transcribe.error.writer.rounded.header.number.is.not.json.representable = il numero arrotondato dell'intestazione non è { $code_json }-rappresentabile

journal.cli.transcribe.error.writer.schema = schema sconosciuto: { $schema }

journal.cli.transcribe.error.writer.segment.meta.must.be.an.object = { $code_segment_meta } deve essere un oggetto

journal.cli.transcribe.error.writer.shape.byte.count.overflows = il conteggio di byte della forma va in overflow

journal.cli.transcribe.error.writer.shape.must.be.row.count.width = la forma deve essere { $code_row_count_256 }

journal.cli.transcribe.error.writer.shape.row.count.is.too.large = il conteggio di righe della forma è troppo grande

journal.cli.transcribe.error.writer.speaker.evidence.multi.fraction.is.required.with.speaker.evidence = { $code_speaker_evidence_multi_fraction } è obbligatorio con { $code_speaker_evidence }

journal.cli.transcribe.error.writer.speaker.evidence.version.is.required.with.speaker.evidence = { $code_speaker_evidence_version } è obbligatorio con { $code_speaker_evidence }

journal.cli.transcribe.error.writer.speaker.must.be.a.string.integer.null.or.absent = il parlante deve essere una stringa, un intero, null o assente

journal.cli.transcribe.error.writer.speech.loud.windows.is.required.when.loud.windows.is = { $code_speech_loud_windows } è obbligatorio quando { $code_loud_windows } è positivo

journal.cli.transcribe.error.writer.stage.path.already.exists = il percorso di preparazione esiste già

journal.cli.transcribe.error.writer.stage.path.is.a.symlink = il percorso di preparazione è un collegamento simbolico

journal.cli.transcribe.error.writer.stage.path.is.not.a.private.directory = il percorso di preparazione non è una directory privata

journal.cli.transcribe.error.writer.stale.npz.unlink.refused = l'unlink dell'npz non aggiornato è stato rifiutato

journal.cli.transcribe.error.writer.statement = enunciato { $index }: { $detail }

journal.cli.transcribe.error.writer.statement.duplicate = l'enunciato { $index } duplica l'id { $id }

journal.cli.transcribe.error.writer.statement.id.is.not.a.transcript.statement.id = l'ID dell'enunciato non è un ID di enunciato della trascrizione

journal.cli.transcribe.error.writer.statement.id.is.outside.i32 = l'ID dell'enunciato è fuori da { $code_i32 }

journal.cli.transcribe.error.writer.statement.id.missing = all'enunciato { $index } manca l'id

journal.cli.transcribe.error.writer.statement.ids.count.does.not.match.shape = il conteggio di { $code_statement_ids } non corrisponde alla forma

journal.cli.transcribe.error.writer.statement.ids.must.be.unique = gli ID degli enunciati devono essere unici

journal.cli.transcribe.error.writer.statement.text.ended.before.its.trimmed.length = il testo dell'enunciato è terminato prima della sua lunghezza tagliata

journal.cli.transcribe.error.writer.statement.text.is.not.valid.utf8 = il testo dell'enunciato non è { $code_utf_8 } valido

journal.cli.transcribe.error.writer.text.must.be.a.nonempty.string = il testo deve essere una stringa non vuota

journal.cli.transcribe.error.writer.turn.audio.duration.s.must.be.non.negative = { $code_audio_duration_s } deve essere non negativo

journal.cli.transcribe.error.writer.turn.boundary.suspect.must.be.true.if.present = { $code_turn_boundary_suspect } deve essere vero se presente

journal.cli.transcribe.error.writer.turn.expected.version.expected.got.version = versione attesa { $expected }, ottenuta { $version }

journal.cli.transcribe.error.writer.turn.field.is.missing = manca { $field }

journal.cli.transcribe.error.writer.turn.field.must.be.finite = { $field } deve essere finito

journal.cli.transcribe.error.writer.turn.invalid = { $code_speaker_turn_evidence } non valido: { $detail }

journal.cli.transcribe.error.writer.turn.range.index.end.s.end.s.exceeds.audio = l'intervallo { $index } { $code_end_s } { $end_s } supera { $code_audio_duration_s } { $audio_duration_s }

journal.cli.transcribe.error.writer.turn.range.index.has.unexpected.keys = l'intervallo { $index } ha chiavi inattese

journal.cli.transcribe.error.writer.turn.range.index.is.an.exact.duplicate = l'intervallo { $index } è un duplicato esatto

journal.cli.transcribe.error.writer.turn.range.index.is.inverted.start.s.start.s = l'intervallo { $index } è invertito: { $code_start_s } { $start_s } > { $code_end_s } { $end_s }

journal.cli.transcribe.error.writer.turn.range.index.must.be.an.object = l'intervallo { $index } deve essere un oggetto

journal.cli.transcribe.error.writer.turn.range.index.start.s.must.be.non.negative = l'intervallo { $index } { $code_start_s } deve essere non negativo

journal.cli.transcribe.error.writer.turn.ranges.must.be.an.array = gli intervalli devono essere un array

journal.cli.transcribe.error.writer.turn.ranges.must.be.sorted.range.index.is.out = gli intervalli devono essere ordinati: l'intervallo { $index } è fuori ordine

journal.cli.transcribe.error.writer.turn.speaker.turns.evidence.has.unexpected.keys = la prova dei turni di parlante ha chiavi inattese

journal.cli.transcribe.error.writer.turn.speaker.turns.evidence.must.be.an.object = la prova dei turni di parlante deve essere un oggetto

journal.cli.transcribe.error.writer.turn.version.must.be.a.string = la versione deve essere una stringa

journal.cli.transcribe.error.writer.zip.central.extra.at.extra.at.error = extra centrale a { $extra_at }: { $error }

journal.cli.transcribe.error.writer.zip.central.header.at.central.header.start.error = intestazione centrale a { $central_header_start }: { $error }

journal.cli.transcribe.error.writer.zip.central.name.at.name.at.error = nome centrale a { $name_at }: { $error }

journal.cli.transcribe.error.writer.zip.central.record.extends.beyond.archive = il registro centrale si estende oltre l'archivio compresso

journal.cli.transcribe.error.writer.zip.central.signature.at.central.header.start = firma centrale a { $central_header_start }

journal.cli.transcribe.error.writer.zip.cp437.scalar = { $code_cp437 } scalare

journal.cli.transcribe.error.writer.zip.flagged.name.is.not.utf8 = il nome contrassegnato non è { $code_utf_8 }

journal.cli.transcribe.error.writer.zip.local.header.at.header.start.error = intestazione locale a { $header_start }: { $error }

journal.cli.transcribe.error.writer.zip.local.signature.at.header.start = firma locale a { $header_start }

journal.cli.transcribe.error.writer.zip.raw.and.exposed.central.populations.disagree = le popolazioni centrali grezze ed esposte non concordano

journal.cli.transcribe.error.writer.zip.raw.central.member.was.lost.from.the.zip = un membro centrale grezzo è stato perso dall'indice ZIP

journal.cli.transcribe.error.writer.zip.repeated.exposed.central.coordinate = coordinata centrale esposta ripetuta

journal.cli.transcribe.error.writer.zip.unicode.path.extra.crc.mismatch = mancata corrispondenza del crc dell'extra di percorso unicode

journal.cli.transcribe.error.writer.zip.unicode.path.extra.is.not.utf8 = l'extra di percorso unicode non è { $code_utf_8 }

journal.cli.transcribe.error.writer.zip.unicode.path.extra.too.small = l'extra di percorso unicode è troppo piccolo

journal.cli.transcribe.error.writer.zip.unproved = il nome del membro zip { $index } non è provato: { $detail }

journal.cli.transcribe.error.writer.zip.unreadable = l'archivio compresso zip è illeggibile: { $detail }

journal.cli.transcribe.error.writer.zip.zip.member.name.collision.name = collisione del nome del membro zip: { $name }

journal.cli.transcribe.feed.failure = l'alimentazione del pool di candidati del parlante non è riuscita per { $path }: { $detail }

journal.cli.transcribe.feed.parse = non è stato possibile analizzare la trascrizione: { $detail }

journal.cli.transcribe.feed.pool.lock = il blocco del pool di candidati non è riuscito: { $detail }

journal.cli.transcribe.feed.pool.read = la lettura del pool di candidati non è riuscita: { $detail }

journal.cli.transcribe.feed.pool.width = mancata corrispondenza della larghezza del centroide del pool di candidati

journal.cli.transcribe.feed.pool.write = la scrittura del pool di candidati non è riuscita: { $detail }

journal.cli.transcribe.feed.read = non è stato possibile leggere la trascrizione: { $detail }

journal.cli.transcribe.feed.unreadable = non è stato possibile leggere la trascrizione completata o gli embedding

journal.cli.transcribe.file.failed = { $file }: la trascrizione non è riuscita ({ $reason }); dettagli: { $output }

journal.cli.transcribe.help = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $syntax_continuation_3 }{ $line_break }{ $line_break }Trascrivi i file audio con { $stt } intercambiabile e analisi nativa del parlante{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }  { $input }            Percorso del file audio nella directory di segmento del diario, ad es.{ $line_break }                        { $sample }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $indent_2 }{ $all_option }{ $gap_17 }Trascrivi in blocco tutti i segmenti audio non elaborati nel{ $line_break }                        diario{ $line_break }{ $indent_2 }{ $redo_option }{ $gap_16 }Rielabora il file, sovrascrivendo gli output esistenti{ $line_break }{ $syntax_continuation_16 }{ $line_break }                        { $stt } backend da usare (sostituisce la configurazione e il predefinito{ $line_break }                        automatico in base alle risorse){ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_9 }Attiva l'output dettagliato{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_11 }Attiva il registro di debug

journal.cli.transcribe.installation.capability.mismatch = la capacità di generazione non corrisponde all'installazione attuale

journal.cli.transcribe.installation.coded = { $code }: { $detail }

journal.cli.transcribe.installation.fd.platform = { $fd }: piattaforma non supportata

journal.cli.transcribe.installation.grant.kind = tipo di concessione di generazione inatteso

journal.cli.transcribe.installation.grant.required = la concessione di generazione autenticata è mancante o duplicata

journal.cli.transcribe.installation.helper.parent = { $helper }: l'eseguibile corrente non ha una directory superiore

journal.cli.transcribe.installation.id.encoding = non-{ $unicode } ID di generazione

journal.cli.transcribe.installation.lock.kind = il blocco di generazione non è un file ordinario

journal.cli.transcribe.installation.metadata.malformed = metadati di generazione malformati

journal.cli.transcribe.installation.metadata.partial = metadati di generazione parziali

journal.cli.transcribe.installation.metadata.required = mancano i metadati di generazione

journal.cli.transcribe.installation.metadata.unverified = i metadati di generazione non hanno una capacità di avvio autenticata

journal.cli.transcribe.installation.obsolete = marcatore obsoleto del descrittore di generazione { $windows }

journal.cli.transcribe.installation.platform = { $code }: { $platform }/{ $architecture }

journal.cli.transcribe.installation.provenance = la provenienza di avvio della generazione non corrisponde a questa radice

journal.cli.transcribe.installation.stamp = { $code }: { $path }: { $error }

journal.cli.transcribe.installation.token.encoding = non-{ $unicode } token di generazione

journal.cli.transcribe.owner.running = il tuo diario è in esecuzione e trascrive le registrazioni da sé. porta invece la registrazione con '{ $import }'.

journal.cli.transcribe.owner.stopped = il tuo diario non è in esecuzione. avvialo con '{ $up }', poi porta la registrazione con '{ $import }'; il tuo diario la trascrive.

journal.cli.transcribe.selection.conflict = { $all_flag } e { $input } si escludono a vicenda

journal.cli.transcribe.selection.missing = fornisci { $input } o { $all_flag }

journal.cli.transcribe.sound.all.windows.failed = l'etichettatura del suono non è riuscita per tutte le finestre: { $detail }

journal.cli.transcribe.sound.engine.absent = il motore ced non è installato

journal.cli.transcribe.sound.failed = l'etichettatura del suono non è riuscita: { $error }

journal.cli.transcribe.sound.helper.executable = il binario dell'ausiliario ced non è eseguibile: { $path }

journal.cli.transcribe.sound.helper.exit = l'ausiliario ced è uscito con { $code }: { $stderr }

journal.cli.transcribe.sound.helper.io = l'I/O dell'ausiliario ced non è riuscito: { $detail }

journal.cli.transcribe.sound.helper.large = la risposta di ced ha superato 64 MiB

journal.cli.transcribe.sound.helper.missing = manca il binario dell'ausiliario ced: { $path }

journal.cli.transcribe.sound.helper.parent = l'eseguibile corrente non ha una directory superiore

journal.cli.transcribe.sound.helper.response = la risposta dell'ausiliario ced non era JSON valido: { $detail }

journal.cli.transcribe.sound.helper.spawn = non è stato possibile avviare l'ausiliario ced: { $detail }

journal.cli.transcribe.sound.helper.timeout = l'ausiliario ced non è uscito entro { $timeout }

journal.cli.transcribe.sound.helper.unresolved = l'ausiliario ced non è risolto: { $detail }

journal.cli.transcribe.sound.helper.windows = CED richiede il chiamante dell'host Windows

journal.cli.transcribe.sound.package.platform = piattaforma CED non supportata { $os }/{ $arch }

journal.cli.transcribe.sound.package.windows.runtime = la verifica del pacchetto CED di Windows richiede un ambiente di esecuzione Windows

journal.cli.transcribe.sound.prepare = non è stato possibile preparare l'audio per l'etichettatura del suono: { $error }

journal.cli.transcribe.sound.probe.helper.exit = l'ausiliario di sonda ced è uscito con { $code }: { $stderr }

journal.cli.transcribe.sound.probe.helper.unexpected = l'ausiliario di sonda ced ha restituito una risposta inattesa: { $response }

journal.cli.transcribe.sound.probe.unexpected = la sonda ced ha restituito una risposta inattesa: { $response }

journal.cli.transcribe.sound.schema = l'etichettatura del suono ha restituito uno schema di risposta inatteso

journal.cli.transcribe.sound.unsupported = etichettatore del suono disattivato: risorse ced non supportate su { $os }/{ $arch }

journal.cli.transcribe.sound.window.count = l'etichettatura del suono ha restituito un numero di finestre inatteso

journal.cli.transcribe.sound.window.ok = l'etichettatura del suono ha restituito una finestra senza il campo { $field }

journal.cli.transcribe.sound.windows.launch = l'avvio dell'ausiliario CED non è riuscito: { $error }

journal.cli.transcribe.sound.windows.library = la libreria della richiesta CED non corrisponde al pacchetto firmato

journal.cli.transcribe.sound.windows.package.bin = l'eseguibile in esecuzione non è nella directory bin del pacchetto

journal.cli.transcribe.sound.windows.package.member = il payload firmato dell'app CED rifiuta { $member }: { $error }

journal.cli.transcribe.sound.windows.package.parent = l'eseguibile in esecuzione non ha una directory che lo contiene

journal.cli.transcribe.sound.windows.package.root = la directory bin del pacchetto non ha una radice di pacchetto

journal.cli.transcribe.sound.windows.probe = l'ausiliario CED ha restituito una risposta di sonda non valida

journal.cli.transcribe.sound.windows.root = { $variable } non è impostato

journal.cli.transcribe.sound.windows.signed = Windows CED richiede l'ausiliario del pacchetto firmato e il suo comando dichiarato

journal.cli.transcribe.stage.confidential.cap = l'audio è { $seconds }s e invierebbe { $bytes } byte; lo STT confidenziale accetta al massimo { $limit } byte

journal.cli.transcribe.stage.confidential.verification.clear = la verifica della trascrizione confidenziale non è stata azzerata per { $path }: { $error }

journal.cli.transcribe.stage.confidential.verification.record = la verifica della trascrizione confidenziale non è stata annotata per { $path }: { $error }

journal.cli.transcribe.stage.embedded.missing.draft = la passata di riconoscimento non ha uno spool di bozza

journal.cli.transcribe.stage.embedded.range.missing = manca l'intervallo del turno unito

journal.cli.transcribe.stage.embedded.scratch = non è stato possibile creare lo scratch incorporato: { $error }

journal.cli.transcribe.stage.output.parent = il percorso della trascrizione non ha una directory superiore

journal.cli.transcribe.stage.owner.deleted.unrecorded = l'originale di sola voce è stato eliminato, ma l'evento di eliminazione non è stato annotato

journal.cli.transcribe.stage.owner.helper.outdated = l'ausiliario di analisi del parlante è precedente agli intervalli di sovrapposizione o ai turni del parlante

journal.cli.transcribe.stage.owner.not.deleted = l'originale di sola voce non è stato eliminato ora: { $outcome }

journal.cli.transcribe.stage.owner.save.or.disable = salva una voce in Parlanti, o disattiva { $setting }

journal.cli.transcribe.stage.owner.save.to.finish = salva una voce in Parlanti per finire questa registrazione filtrata

journal.cli.transcribe.summary = { $processed } elaborati, { $skipped } saltati (già trascritti)

journal.cli.transcribe.summary.deferred = { $processed } elaborati, { $skipped } saltati (già trascritti), { $deferred } rinviati (il fornitore non è pronto, si riproverà)

journal.cli.transcribe.summary.deferred.failed = { $processed } elaborati, { $skipped } saltati (già trascritti), { $deferred } rinviati (il fornitore non è pronto, si riproverà), { $failed } falliti

journal.cli.transcribe.summary.failed = { $processed } elaborati, { $skipped } saltati (già trascritti), { $failed } falliti

journal.cli.transcribe.usage.error = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $syntax_continuation_3 }{ $line_break }{ $command }: errore: { $message }
