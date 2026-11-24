journal.setup.cancel.admission = admisión del instalador no disponible

journal.setup.cancel.attempt = intento del instalador no disponible

journal.setup.cancel.busy = el instalador todavía tiene su reserva

journal.setup.cancel.changed = cambió el intento de instalación

journal.setup.cancel.failed = no se pudo cancelar

journal.setup.cancel.identity = la identidad del instalador no está disponible

journal.setup.cancel.owner = el titular del instalador no está disponible

journal.setup.cleanup.legacy.log = no se pudo limpiar el alias de registro heredado: { $error }

journal.setup.cleanup.retired.support = no se pudo limpiar el estado del portal de soporte retirado: { $error }

journal.setup.doctor.blocked = falló el bloqueador de doctor

journal.setup.doctor.failed.status = doctor terminó con estado { $status }

journal.setup.doctor.incomplete = el flujo JSONL de doctor terminó sin { $event }

journal.setup.doctor.silent = doctor no informó progreso durante { $seconds }s

journal.setup.doctor.timeout = doctor agotó el tiempo tras { $seconds }s

journal.setup.doctor.unreadable = no se pudo analizar el JSON de doctor

journal.setup.downloads.cancel = cancelar

journal.setup.downloads.cancelled = cancelada

journal.setup.downloads.complete = listo

journal.setup.downloads.error = no se pudo descargar. vuelve a intentarlo.

journal.setup.downloads.idle = sin iniciar

journal.setup.downloads.progress = descargando…

journal.setup.downloads.retry = reintentar

journal.setup.downloads.speech = reconocimiento de voz

journal.setup.downloads.thinking = razonamiento local

journal.setup.drive.unsupported = tu diario no puede correr en esta unidad porque la unidad no puede guardar archivos como tu diario lo necesita. usa una unidad formateada para mac para tu diario.

journal.setup.fits.app = corre en cada dispositivo que eliges y recibe lo que compartes con él. todo ello entra en tu diario.

journal.setup.fits.app.term = la app de { $platform_name }

journal.setup.fits.journal.computer = vive en la computadora de tu diario y guarda todo lo que entra en ella.

journal.setup.fits.journal.phone = vive en este teléfono y guarda todo lo que entra en él.

journal.setup.fits.thinking = ayuda a tu diario a dar sentido a lo que guarda. elige abajo dónde ocurre, o más tarde.

journal.setup.fits.thinking.term = razonamiento

journal.setup.fits.title = cómo encaja todo

journal.setup.folder.change = cambiar carpeta

journal.setup.folder.continue = continuar

journal.setup.folder.help = dónde se guarda tu diario en la computadora de tu diario.

journal.setup.folder.title = carpeta del diario

journal.setup.footer.computer = tu diario se guarda en la computadora de tu diario. el razonamiento usa la opción que elijas.

journal.setup.footer.phone = tu diario se guarda en este teléfono. el razonamiento usa la opción que elijas.

journal.setup.heading = tu diario

journal.setup.help.text = ¿te atascaste, o tienes una pregunta?

journal.setup.identity.ca.invalid = CA local no válida: { $detail }

journal.setup.identity.ca.randomness.unavailable = CSPRNG del sistema no disponible

journal.setup.identity.foreign = no se pudo verificar esta instalación.{ $paragraph }detalles: { $error }{ $paragraph }otra instalación de { $platform } configuró las herramientas de línea de comandos o el soporte en segundo plano de esta computadora, y nada cambió.

journal.setup.identity.invalid = estado de vinculación local no válido: { $detail }

journal.setup.identity.io.error = { $path }: { $error }

journal.setup.identity.locations = no se pudo verificar esta instalación.{ $paragraph }detalles: { $error }{ $paragraph }las ubicaciones de recuperación de la configuración inicial no están disponibles: { $location_error }

journal.setup.identity.lock.timeout = no se pudo adquirir el bloqueo para { $path } dentro de { $seconds }s

journal.setup.identity.mark.asset.invalid = recurso de marca no válido: { $detail }

journal.setup.identity.mark.jid.invalid = ID del diario no válido: { $detail }

journal.setup.identity.no.candidate = no hay un candidato de identidad de vinculación preparado

journal.setup.identity.program.folder = Reinstalar o desinstalar elimina esta carpeta del programa. Elige una ubicación del diario fuera de ella. Si tu diario ya está ahí, cierra la app del diario. Si la configuración inicial terminó, ejecuta `solstone journal service stop`, luego `solstone journal service uninstall` y espera a que tenga éxito. Mueve toda la carpeta del diario fuera de la carpeta del programa antes de reinstalar o desinstalar, y luego selecciona su nueva ubicación con `solstone journal setup --accept-existing-journal --journal "D:\journal"`, reemplazando D:\journal por su nueva ubicación.

journal.setup.identity.recover = no se pudo verificar esta instalación.{ $paragraph }detalles: { $error }{ $paragraph }para recuperar, detén el servicio, quita los artefactos de configuración inicial de esta instalación y ejecuta `{ $setup }` de nuevo:{ $stop_service }{ $steps }{ $paragraph }tu diario en sí queda intacto. ninguno de estos guarda tus recuerdos.

journal.setup.identity.spki.invalid = SPKI del diario no válido: { $detail }

journal.setup.identity.state.ca.bundle.is.not.a.directory = estado de vinculación local no válido: el paquete CA no es un directorio

journal.setup.identity.state.candidate.ca.is.not.a.directory = estado de vinculación local no válido: la CA candidata no es un directorio

journal.setup.identity.state.candidate.cert.pem = estado de vinculación local no válido: { $candidate_cert }: { $error }

journal.setup.identity.state.candidate.directory = estado de vinculación local no válido: directorio candidato: { $error }

journal.setup.identity.state.candidate.private.pem = estado de vinculación local no válido: { $candidate_key }: { $error }

journal.setup.identity.state.cert.pem = estado de vinculación local no válido: { $cert_file }: { $error }

journal.setup.identity.state.identity.directory = estado de vinculación local no válido: directorio de identidad: { $error }

journal.setup.identity.state.private.pem = estado de vinculación local no válido: { $key_file }: { $error }

journal.setup.identity.state.state.instance.id.does.not.match.the.ca.public.key = estado de vinculación local no válido: el estado { $identity_key } no coincide con la clave pública de la CA

journal.setup.identity.state.state.json = estado de vinculación local no válido: { $state_file }: { $error }

journal.setup.identity.state.state.json.instance.id.is.missing = estado de vinculación local no válido: { $state_file } { $identity_key } falta

journal.setup.identity.state.state.json.locked.at.is.missing = estado de vinculación local no válido: { $state_file } { $locked_key } falta

journal.setup.identity.state.state.json.must.be.an.object = estado de vinculación local no válido: { $state_file } debe ser un objeto

journal.setup.identity.uncertain = no se pudo verificar esta instalación.{ $paragraph }detalles: { $error }{ $paragraph }la configuración inicial no pudo determinar a qué instalación pertenecen las herramientas de línea de comandos o el soporte en segundo plano. déjalos en su lugar e incluye estos detalles en una solicitud de soporte.

journal.setup.identity.windows = no se pudo verificar esta instalación.{ $paragraph }detalles: { $error }{ $paragraph }para recuperar, primero ejecuta `{ $uninstall }`. si falla, detente aquí e incluye estos detalles en una solicitud de soporte. si sale bien, ejecuta estos comandos en { $shell } y luego ejecuta `{ $setup }` de nuevo:{ $commands }{ $paragraph }tu diario en sí queda intacto. ninguno de estos guarda tus recuerdos.

journal.setup.installer.cleanup.failed = no se pudo limpiar la admisión del instalador: { $error }

journal.setup.installer.control.timed.out = se agotó el tiempo del control del instalador

journal.setup.installer.control.unavailable = el control del instalador no está disponible

journal.setup.installer.directory.unavailable = el directorio del instalador no está disponible

journal.setup.installer.exited = el instalador salió antes de la admisión ({ $code })

journal.setup.installer.identity.unavailable = la identidad del instalador no está disponible

journal.setup.installer.job.authority = la autoridad de Job del instalador no está disponible

journal.setup.installer.job.cleanup = no se pudo limpiar el Job del instalador: { $error }

journal.setup.installer.lease.busy = la reserva del instalador está ocupada

journal.setup.installer.process.identity.invalid = la identidad del proceso del instalador no es válida

journal.setup.installer.process.identity.unavailable = la identidad del proceso del instalador no está disponible

journal.setup.installer.process.observation.unavailable = la observación del proceso del instalador no está disponible

journal.setup.installer.registry.unavailable = el registro del instalador no está disponible

journal.setup.installer.still.running = el instalador todavía está en ejecución

journal.setup.installer.stopped = el instalador se detuvo

journal.setup.installer.timed.out = se agotó el tiempo de admisión del instalador

journal.setup.installer.tree.not.reaped = no se recogió el árbol de procesos: { $reason }; sobrevivientes={ $survivors }

journal.setup.journal.aborted = quien la usa abortó la configuración inicial

journal.setup.journal.existing = { $setup }: no se puede continuar en modo no interactivo - { $path } ya contiene datos del diario.{ $line }La configuración inicial no reclamará automáticamente un diario existente.{ $paragraph }Reintenta con una de estas:{ $indent }{ $accept }{ $indent }{ $new }{ $paragraph }Escape interactivo:{ $indent }{ $verb }{ $paragraph }Ejecuta { $explain } para la lista completa de pasos.

journal.setup.journal.existing.installer = { $setup }: no se puede continuar en modo no interactivo - { $path } ya contiene datos del diario.{ $line }La configuración inicial no reclamará automáticamente un diario existente.{ $paragraph }Reintenta con una de estas:{ $indent }{ $accept }{ $indent }{ $new }{ $paragraph }Escape interactivo:{ $indent }{ $verb }{ $paragraph }Después de cualquiera de estas, ejecuta otra vez el mismo comando { $installer } para terminar la instalación.{ $paragraph }Ejecuta { $explain } para la lista completa de pasos.

journal.setup.journal.path.file = se esperaba un directorio en { $path }; hay un archivo regular. Ejecútalo de nuevo con { $option }.

journal.setup.login.help = iniciar automáticamente después de que inicies sesión en la computadora de tu diario.

journal.setup.login.label = iniciar tu diario cuando inicias sesión

journal.setup.mark.error = no se pudo cargar la marca de tu diario. vuelve a intentarlo.

journal.setup.mark.help = una marca única de tu diario. un teléfono u otra computadora que vincules también la muestra, para que puedas revisarla.

journal.setup.mark.permanence = continuar fija esta como la marca de tu diario de forma definitiva.

journal.setup.mark.title = la marca de tu diario

journal.setup.mark.try.another = probar otra

journal.setup.refusal.action.is.required = se requiere una acción

journal.setup.refusal.attempt.id.is.required = { $attempt_id_value } es obligatorio

journal.setup.refusal.bad.request = Solicitud incorrecta

journal.setup.refusal.choice.is.required = se requiere una elección

journal.setup.refusal.choose.local.thinking.before.starting.its.download = elige el razonamiento local antes de empezar su descarga

journal.setup.refusal.choose.thinking.before.finishing.setup = elige el razonamiento antes de terminar la configuración inicial

journal.setup.refusal.clear.your.own.endpoint.first.to.run.the.bundled.local.model = borra tu propio endpoint primero para ejecutar el modelo local incluido.

journal.setup.refusal.conflict = Conflicto

journal.setup.refusal.forbidden = Prohibido

journal.setup.refusal.identity.proposal.conflict = conflicto de propuesta de identidad

journal.setup.refusal.internal.server.error = Error interno del servidor

journal.setup.refusal.invalid.timezone = zona horaria no válida

journal.setup.refusal.journal.id.already.locked = el id del diario ya está bloqueado

journal.setup.refusal.journal.id.must.be.locked.before.setup.can.finish = el id del diario debe estar bloqueado antes de que termine la configuración inicial

journal.setup.refusal.lane.must.be.one.of.byo.confidential.local = lane debe ser uno de: { $byo_value }, confidential, local

journal.setup.refusal.local.thinking.is.not.available.on.your.journal.s.computer = el razonamiento local no está disponible en la computadora de tu diario

journal.setup.refusal.malformed.json.request = solicitud json mal formada

journal.setup.refusal.malformed.lock.request = solicitud de bloqueo mal formada

journal.setup.refusal.not.found = no se encontró

journal.setup.refusal.owner.identity.failed = no se pudo guardar el perfil de dueño de tu diario

journal.setup.refusal.proposal.id.is.required = { $proposal_id_value } es obligatorio

journal.setup.refusal.retention.days.must.be.a.positive.integer = retention_days debe ser un entero positivo

journal.setup.refusal.retry.continues.the.same.backup.restore = reintento sigue la misma restauración de copia de seguridad.

journal.setup.refusal.service.unavailable = Servicio no disponible

journal.setup.refusal.settings.lock.unavailable = bloqueo de configuración no disponible

journal.setup.refusal.settings.read.failed = no se pudo leer la configuración

journal.setup.refusal.settings.write.failed = no se pudo escribir la configuración

journal.setup.refusal.setup.routes.require.a.localhost.connection = las rutas de la configuración inicial requieren una conexión a localhost

journal.setup.refusal.setup.target.use.was.refused = se rechazó el uso del destino de la configuración inicial

journal.setup.refusal.speech.recognition.needs.repair = el reconocimiento de voz necesita reparación

journal.setup.refusal.the.restore.didn.t.complete = la restauración no se completó

journal.setup.refusal.thinking.choice.is.required.before.setup.can.finish = se requiere la elección de razonamiento antes de que termine la configuración inicial

journal.setup.refusal.thinking.download.could.not.start = no se pudo iniciar la descarga del razonamiento

journal.setup.refusal.this.journal.is.still.being.restored.and.isn.t.ready.to.finish.setup = este diario todavía se está restaurando y no está listo para terminar la configuración inicial.

journal.setup.refusal.this.journal.is.still.being.restored.and.isn.t.ready.to.use = este diario todavía se está restaurando y no está listo para usarse.

journal.setup.refusal.this.journal.isn.t.ready.to.use = este diario no está listo para usarse.

journal.setup.refusal.unknown.action = acción desconocida

journal.setup.refusal.unknown.thinking.choice = elección de razonamiento desconocida

journal.setup.refusal.whole.journal.restore.is.pending = la restauración de todo el diario está pendiente

journal.setup.refusal.your.journal.s.identity.couldn.t.be.set.up = no se pudo configurar la identidad de tu diario.

journal.setup.refusal.your.journal.settings.could.not.be.read = no se pudo leer la configuración de tu diario

journal.setup.refusal.your.thinking.choice.changed.review.it.before.trying.again = cambió tu elección de razonamiento; revísala antes de volver a intentarlo

journal.setup.refusal.your.thinking.settings.changed.open.thinking.to.choose.what.to.use = cambió tu configuración de razonamiento; abre razonamiento para elegir qué usar

journal.setup.service.failed = no se pudo iniciar el servicio (salida { $status })

journal.setup.speech.cannot.start = no se pudo iniciar la instalación del habla

journal.setup.speech.check.progress = revisa el progreso del habla en un momento.

journal.setup.speech.retry.unavailable = el reintento del habla no está disponible.

journal.setup.speech.starting = la instalación del habla está empezando

journal.setup.speech.unavailable = reintento no disponible

journal.setup.step.exit = el paso { $step } salió con código { $status }

journal.setup.step.failed = falló el paso

journal.setup.step.timeout = el paso { $step } agotó el tiempo tras { $seconds }s

journal.setup.step.warning = aviso del paso

journal.setup.thinking.change = cambiar

journal.setup.thinking.check.unknown = no se pudo revisar la configuración local.

journal.setup.thinking.choose.later = elige el razonamiento cuando quieras. tu diario guarda todo hasta entonces.

journal.setup.thinking.default.line = en la computadora de tu diario · { $size }

journal.setup.thinking.later = decidir después

journal.setup.thinking.size.unknown = el tamaño de la descarga no está disponible

journal.setup.welcome.body = la app de { $platform_name } recibe lo que compartes con ella, y todo ello entra en tu diario.

journal.setup.welcome.title = te damos la bienvenida a { $platform_name }.

journal.setup.wrapper.legacy = el servicio todavía arranca mediante { $path }

journal.setup.wrapper.legacy.retry = la configuración inicial quita ese envoltorio al arrancar el servicio por { $path }.

journal.setup.wrapper.provision = no se pudo aprovisionar el envoltorio de { $command } en { $path } ({ $kind }: { $error })

journal.setup.wrapper.provision.remedy = corrige los permisos de ~/.local/bin y vuelve a ejecutar `solstone journal setup`, o invoca solstone directamente desde el entorno de ejecución

journal.setup.wrapper.retire.failed = no se pudieron quitar las herramientas de línea de comandos de la versión anterior: { $error }

journal.setup.wrapper.retry = la configuración inicial vuelve a intentarlo la próxima vez que se ejecuta.

journal.window.body.accessible.name.body.views = vistas del cuerpo

journal.window.body.accessible.name.close = cerrar

journal.window.body.accessible.name.days.your.journal.holds.body.data = días en que tu diario guarda datos del cuerpo

journal.window.body.accessible.name.empty.day = día vacío

journal.window.body.button.label.delete = eliminar

journal.window.body.count.day = { $quantity ->
    [one] { $count } día
    [many] { $count } de días
   *[other] { $count } días
}

journal.window.body.count.entry = { $quantity ->
    [one] { $count } entrada
    [many] { $count } de entradas
   *[other] { $count } entradas
}

journal.window.body.count.interval = { $quantity ->
    [one] { $count } intervalo
    [many] { $count } de intervalos
   *[other] { $count } intervalos
}

journal.window.body.count.reading = { $quantity ->
    [one] { $count } lectura
    [many] { $count } de lecturas
   *[other] { $count } lecturas
}

journal.window.body.date.clock.afternoon = { $hour }:{ $minute } p. m.

journal.window.body.date.clock.morning = { $hour }:{ $minute } a. m.

journal.window.body.date.clock.range = { $first_clock } – { $last_clock }

journal.window.body.date.full = {$month}{ $slot1 }{$day} de{ $slot2 }{$year}

journal.window.body.date.month.year = {$month} de{ $slot1 }{$year}

journal.window.body.date.short = {$month}{ $slot1 }{$day}

journal.window.body.date.time.before = {$date},{ $slot1 }a las {$time}

journal.window.body.date.with.clock = {$month}{ $slot1 }{$day} de{ $slot2 }{$year},{ $slot3 }{$time}

journal.window.body.day.invalid = no se pudo usar ese día.

journal.window.body.day.invalid.detail = Día no válido

journal.window.body.day.nearby.with.data = días cercanos con datos: { $nearby_days }

journal.window.body.document.dated = para esta fecha: { $name } { $value }

journal.window.body.document.dated.list = { $quantity ->
    [one] documento diario de { $source } para esta fecha: { $leading } y { $last }
    [many] documentos diarios de { $source } para esta fecha: { $leading } y { $last }
   *[other] documentos diarios de { $source } para esta fecha: { $leading } y { $last }
}

journal.window.body.document.dated.one = { $quantity ->
    [one] documento diario de { $source } para esta fecha: { $last }
    [many] documentos diarios de { $source } para esta fecha: { $last }
   *[other] documentos diarios de { $source } para esta fecha: { $last }
}

journal.window.body.document.experience.continuing.generic = experiencia que continúa: { $name }, { $start } a { $end }

journal.window.body.document.experience.continuing.generic.recorder = experiencia que continúa: { $name }, { $start } a { $end } ({ $recorder })

journal.window.body.document.experience.continuing.named = { $kind } que continúa: { $name }, { $start } a { $end }

journal.window.body.document.experience.continuing.named.recorder = { $kind } que continúa: { $name }, { $start } a { $end } ({ $recorder })

journal.window.body.document.experience.generic = experiencia: { $name }, { $start } a { $end }

journal.window.body.document.experience.generic.recorder = experiencia: { $name }, { $start } a { $end } ({ $recorder })

journal.window.body.document.experience.generic.recorder.words = experiencia: { $name }, { $start } a { $end } ({ $recorder }): { $words }

journal.window.body.document.experience.generic.words = experiencia: { $name }, { $start } a { $end }: { $words }

journal.window.body.document.experience.named = { $kind }: { $name }, { $start } a { $end }

journal.window.body.document.experience.named.recorder = { $kind }: { $name }, { $start } a { $end } ({ $recorder })

journal.window.body.document.experience.named.recorder.words = { $kind }: { $name }, { $start } a { $end } ({ $recorder }): { $words }

journal.window.body.document.experience.named.words = { $kind }: { $name }, { $start } a { $end }: { $words }

journal.window.body.document.heading = cuerpo el { $weekday }, { $month } { $day }, { $year }

journal.window.body.document.heading.raw = cuerpo el { $day }

journal.window.body.document.tally.count.plain.at = { $quantity ->
    [one] { $name }: { $count } intervalo, { $start }
    [many] { $name }: { $count } de intervalos, { $start }
   *[other] { $name }: intervalos { $count }, { $start }
}

journal.window.body.document.tally.count.plain.count = { $quantity ->
    [one] { $name }: { $count } intervalo
    [many] { $name }: { $count } de intervalos
   *[other] { $name }: intervalos { $count }
}

journal.window.body.document.tally.count.plain.range = { $quantity ->
    [one] { $name }: { $count } intervalo, { $start } a { $end }
    [many] { $name }: { $count } de intervalos, { $start } a { $end }
   *[other] { $name }: intervalos { $count }, { $start } a { $end }
}

journal.window.body.document.tally.count.recorder.at = { $quantity ->
    [one] { $name }: { $count } intervalo, { $start } ({ $recorder })
    [many] { $name }: { $count } de intervalos, { $start } ({ $recorder })
   *[other] { $name }: intervalos { $count }, { $start } ({ $recorder })
}

journal.window.body.document.tally.count.recorder.count = { $quantity ->
    [one] { $name }: { $count } intervalo ({ $recorder })
    [many] { $name }: { $count } de intervalos ({ $recorder })
   *[other] { $name }: intervalos { $count } ({ $recorder })
}

journal.window.body.document.tally.count.recorder.range = { $quantity ->
    [one] { $name }: { $count } intervalo, { $start } a { $end } ({ $recorder })
    [many] { $name }: { $count } de intervalos, { $start } a { $end } ({ $recorder })
   *[other] { $name }: intervalos { $count }, { $start } a { $end } ({ $recorder })
}

journal.window.body.document.tally.logged.owner.at = { $quantity ->
    [one] { $name }: { $count } entrada, { $start } (ingresada por el dueño)
    [many] { $name }: { $count } de entradas, { $start } (ingresadas por el dueño)
   *[other] { $name }: entradas { $count }, { $start } (ingresadas por el dueño)
}

journal.window.body.document.tally.logged.owner.count = { $quantity ->
    [one] { $name }: { $count } entrada (ingresada por el dueño)
    [many] { $name }: { $count } de entradas (ingresadas por el dueño)
   *[other] { $name }: entradas { $count } (ingresadas por el dueño)
}

journal.window.body.document.tally.logged.owner.range = { $quantity ->
    [one] { $name }: { $count } entrada, { $start } a { $end } (ingresada por el dueño)
    [many] { $name }: { $count } de entradas, { $start } a { $end } (ingresadas por el dueño)
   *[other] { $name }: entradas { $count }, { $start } a { $end } (ingresadas por el dueño)
}

journal.window.body.document.tally.logged.owner.recorder.at = { $quantity ->
    [one] { $name }: { $count } entrada, { $start } (ingresada por el dueño en { $recorder })
    [many] { $name }: { $count } de entradas, { $start } (ingresadas por el dueño en { $recorder })
   *[other] { $name }: entradas { $count }, { $start } (ingresadas por el dueño en { $recorder })
}

journal.window.body.document.tally.logged.owner.recorder.count = { $quantity ->
    [one] { $name }: { $count } entrada (ingresada por el dueño en { $recorder })
    [many] { $name }: { $count } de entradas (ingresadas por el dueño en { $recorder })
   *[other] { $name }: entradas { $count } (ingresadas por el dueño en { $recorder })
}

journal.window.body.document.tally.logged.owner.recorder.range = { $quantity ->
    [one] { $name }: { $count } entrada, { $start } a { $end } (ingresada por el dueño en { $recorder })
    [many] { $name }: { $count } de entradas, { $start } a { $end } (ingresadas por el dueño en { $recorder })
   *[other] { $name }: entradas { $count }, { $start } a { $end } (ingresadas por el dueño en { $recorder })
}

journal.window.body.document.tally.logged.plain.at = { $quantity ->
    [one] { $name }: { $count } entrada, { $start }
    [many] { $name }: { $count } de entradas, { $start }
   *[other] { $name }: entradas { $count }, { $start }
}

journal.window.body.document.tally.logged.plain.count = { $quantity ->
    [one] { $name }: { $count } entrada
    [many] { $name }: { $count } de entradas
   *[other] { $name }: entradas { $count }
}

journal.window.body.document.tally.logged.plain.range = { $quantity ->
    [one] { $name }: { $count } entrada, { $start } a { $end }
    [many] { $name }: { $count } de entradas, { $start } a { $end }
   *[other] { $name }: entradas { $count }, { $start } a { $end }
}

journal.window.body.document.tally.logged.recorder.at = { $quantity ->
    [one] { $name }: { $count } entrada, { $start } (registrada por { $recorder })
    [many] { $name }: { $count } de entradas, { $start } (registradas por { $recorder })
   *[other] { $name }: entradas { $count }, { $start } (registradas por { $recorder })
}

journal.window.body.document.tally.logged.recorder.count = { $quantity ->
    [one] { $name }: { $count } entrada (registrada por { $recorder })
    [many] { $name }: { $count } de entradas (registradas por { $recorder })
   *[other] { $name }: entradas { $count } (registradas por { $recorder })
}

journal.window.body.document.tally.logged.recorder.range = { $quantity ->
    [one] { $name }: { $count } entrada, { $start } a { $end } (registrada por { $recorder })
    [many] { $name }: { $count } de entradas, { $start } a { $end } (registradas por { $recorder })
   *[other] { $name }: entradas { $count }, { $start } a { $end } (registradas por { $recorder })
}

journal.window.body.document.tally.reading.plain.at = { $quantity ->
    [one] { $name }: { $count } lectura, { $start }
    [many] { $name }: { $count } de lecturas, { $start }
   *[other] { $name }: lecturas { $count }, { $start }
}

journal.window.body.document.tally.reading.plain.count = { $quantity ->
    [one] { $name }: { $count } lectura
    [many] { $name }: { $count } de lecturas
   *[other] { $name }: lecturas { $count }
}

journal.window.body.document.tally.reading.plain.range = { $quantity ->
    [one] { $name }: { $count } lectura, { $start } a { $end }
    [many] { $name }: { $count } de lecturas, { $start } a { $end }
   *[other] { $name }: lecturas { $count }, { $start } a { $end }
}

journal.window.body.document.tally.reading.recorder.at = { $quantity ->
    [one] { $name }: { $count } lectura, { $start } ({ $recorder })
    [many] { $name }: { $count } de lecturas, { $start } ({ $recorder })
   *[other] { $name }: lecturas { $count }, { $start } ({ $recorder })
}

journal.window.body.document.tally.reading.recorder.count = { $quantity ->
    [one] { $name }: { $count } lectura ({ $recorder })
    [many] { $name }: { $count } de lecturas ({ $recorder })
   *[other] { $name }: lecturas { $count } ({ $recorder })
}

journal.window.body.document.tally.reading.recorder.range = { $quantity ->
    [one] { $name }: { $count } lectura, { $start } a { $end } ({ $recorder })
    [many] { $name }: { $count } de lecturas, { $start } a { $end } ({ $recorder })
   *[other] { $name }: lecturas { $count }, { $start } a { $end } ({ $recorder })
}

journal.window.body.document.tally.stage.plain.at = { $quantity ->
    [one] etapa de sueño { $name }: { $count } intervalo, { $start }
    [many] etapa de sueño { $name }: { $count } de intervalos, { $start }
   *[other] etapa de sueño { $name }: intervalos { $count }, { $start }
}

journal.window.body.document.tally.stage.plain.count = { $quantity ->
    [one] etapa de sueño { $name }: { $count } intervalo
    [many] etapa de sueño { $name }: { $count } de intervalos
   *[other] etapa de sueño { $name }: intervalos { $count }
}

journal.window.body.document.tally.stage.plain.range = { $quantity ->
    [one] etapa de sueño { $name }: { $count } intervalo, { $start } a { $end }
    [many] etapa de sueño { $name }: { $count } de intervalos, { $start } a { $end }
   *[other] etapa de sueño { $name }: intervalos { $count }, { $start } a { $end }
}

journal.window.body.document.tally.stage.recorder.at = { $quantity ->
    [one] etapa de sueño { $name }: { $count } intervalo, { $start } ({ $recorder })
    [many] etapa de sueño { $name }: { $count } de intervalos, { $start } ({ $recorder })
   *[other] etapa de sueño { $name }: intervalos { $count }, { $start } ({ $recorder })
}

journal.window.body.document.tally.stage.recorder.count = { $quantity ->
    [one] etapa de sueño { $name }: { $count } intervalo ({ $recorder })
    [many] etapa de sueño { $name }: { $count } de intervalos ({ $recorder })
   *[other] etapa de sueño { $name }: intervalos { $count } ({ $recorder })
}

journal.window.body.document.tally.stage.recorder.range = { $quantity ->
    [one] etapa de sueño { $name }: { $count } intervalo, { $start } a { $end } ({ $recorder })
    [many] etapa de sueño { $name }: { $count } de intervalos, { $start } a { $end } ({ $recorder })
   *[other] etapa de sueño { $name }: intervalos { $count }, { $start } a { $end } ({ $recorder })
}

journal.window.body.document.words.owner = en palabras del dueño: { $words }

journal.window.body.document.words.source = en palabras de la fuente: { $words }

journal.window.body.empty.workouts.import = todavía no hay nada aquí. trae tus entrenamientos desde una descarga de Strava en la { $import_link_start }página de importación{ $import_link_end }.

journal.window.body.empty.workouts.import.resources = si tu diario corre en una computadora, también puedes traer una exportación de Apple Health en la página de importación o datos de Oura desde la línea de comandos de esa computadora. { $docs_link_start }leer sobre las importaciones del cuerpo{ $docs_link_end }.

journal.window.body.error.day.detail = Día no válido

journal.window.body.error.day.invalid = no se pudo usar ese día.

journal.window.body.error.hidden.body = El cuerpo debe ser un objeto JSON

journal.window.body.error.hidden.days.empty = los días deben nombrar al menos un día

journal.window.body.error.hidden.days.list = los días deben ser una lista de días

journal.window.body.error.hidden.hidden = oculto debe ser verdadero o falso

journal.window.body.error.hidden.unreadable = tu diario no puede leer qué días ocultaste.

journal.window.body.error.hidden.unreadable.detail = no se pueden leer los días ocultos del cuerpo

journal.window.body.error.hidden.unsaved = no se pudo guardar ese cambio.

journal.window.body.error.invalid.request = no se pudo usar uno de esos valores.

journal.window.body.error.limit.integer = el límite debe ser un entero

journal.window.body.error.limit.minimum = el límite debe ser al menos 1

journal.window.body.error.month.invalid = Mes no válido

journal.window.body.error.unreadable = la app del cuerpo no pudo leer tu diario.

journal.window.body.error.window.bounds = La ventana requiere marcas de tiempo ISO de inicio y de fin válidas.

journal.window.body.error.window.order = El fin de la ventana debe ser posterior al inicio.

journal.window.body.error.window.span = { $quantity ->
    [one] El intervalo de la ventana debe ser de { $maximum_days } día o menos.
    [many] El intervalo de la ventana debe ser de { $maximum_days } días o menos.
   *[other] El intervalo de la ventana debe ser de { $maximum_days } días o menos.
}

journal.window.body.experience.continuing.generic = experiencia que continúa

journal.window.body.experience.continuing.named = { $experience_kind } que continúa

journal.window.body.experience.fallback = experiencia

journal.window.body.grid.empty = {$date}{ $slot1 }· nada aquí

journal.window.body.grid.held = {$date}{ $slot1 }· guardado en una importación anterior

journal.window.body.grid.held.month = {$date}{ $slot1 }· en un mes que una importación anterior guarda

journal.window.body.grid.hidden = {$date}{ $slot1 }· ocultaste este día

journal.window.body.grid.source.all = { $date } · { $slot1 }, { $slot2 } y { $slot3 }

journal.window.body.grid.source.apple = { $date } · { $slot1 }

journal.window.body.grid.source.apple.oura = { $date } · { $slot1 } y { $slot2 }

journal.window.body.grid.source.apple.strava = { $date } · { $slot1 } y { $slot2 }

journal.window.body.grid.source.oura = { $date } · { $slot1 }

journal.window.body.grid.source.oura.strava = { $date } · { $slot1 } y { $slot2 }

journal.window.body.grid.source.strava = { $date } · { $slot1 }

journal.window.body.hide.copy.grid.foot.striped.days.are.ones.you.hid = los días rayados son los que ocultaste.

journal.window.body.hide.copy.hide.failed.couldn.t.hide.this.day.nothing = no se pudo ocultar este día. no cambió nada.

journal.window.body.hide.copy.hide.hide.this.day = ocultar este día

journal.window.body.hide.copy.mark.head.you.hid.this.day.s.body = ocultaste los datos del cuerpo de este día

journal.window.body.hide.copy.mark.line1.it.s.still.in.your.journal = sigue en tu diario y permanece oculto cuando lo importas o lo sincronizas de nuevo.

journal.window.body.hide.copy.mark.line2.search.and.the.activities.page.leave = la búsqueda y la página de actividades lo dejan fuera, y los agentes conectados no pueden alcanzarlo. una noche que empieza este día se oculta con él, también tras la medianoche.

journal.window.body.hide.copy.mark.unhide.unhide.this.day = desocultar este día

journal.window.body.hide.copy.no.answer.hide.it.may.already.be.hidden.reload = puede que ya esté oculto. recarga para revisar.

journal.window.body.hide.copy.no.answer.unhide.it.may.already.be.unhidden.reload = puede que ya se haya desocultado. recarga para revisar.

journal.window.body.hide.copy.no.answer.your.journal.didn.t.answer = tu diario no respondió

journal.window.body.hide.copy.receipt.message.it.stays.hidden.when.you.import = permanece oculto cuando lo importas o lo sincronizas de nuevo.

journal.window.body.hide.copy.receipt.no.search.search.and.connected.agents.may.still = la búsqueda y los agentes conectados todavía pueden encontrarlo.

journal.window.body.hide.copy.try.again.try.again = reintentar

journal.window.body.hide.copy.unhide.failed.couldn.t.unhide.this.day.nothing = no se pudo desocultar este día. no cambió nada.

journal.window.body.hide.copy.unhide.unhide = desocultar

journal.window.body.hide.copy.unreadable.your.journal.can.t.read.which = tu diario no puede leer qué días ocultaste, así que aquí se muestra cada día y no puedes ocultar ni desocultar uno. hasta que pueda, la búsqueda y los agentes conectados omiten todos tus datos del cuerpo.

journal.window.body.hide.data.from.day = ocultar los datos del cuerpo de { $day }

journal.window.body.hide.data.receipt = los datos del cuerpo de { $day } están ocultos.

journal.window.body.hide.previous.day.carryover = el resto de una noche o de un entrenamiento de { $hidden_day_link }, un día que ocultaste, también está oculto aquí.

journal.window.body.import.again.apple.health = vuelve a importar desde una exportación nueva de Apple Health para traerlo.

journal.window.body.import.again.dexcom = ahora no hay un importador para Dexcom Clarity, así que se queda como está.

journal.window.body.import.again.oura = vuelve a importar desde tu cuenta de Oura para traerlo.

journal.window.body.import.old.day = una importación{ $slot1 }{$source}{ $slot2 }anterior guarda los datos del cuerpo de este día, conservados pero no leídos aquí.{ $slot3 }{$action}

journal.window.body.import.old.listing = {$source}{ $slot1 }de{ $slot2 }{$span}, conservado pero no leído aquí.{ $slot3 }{$action}

journal.window.body.import.old.month = una importación{ $slot1 }{$source}{ $slot2 }anterior guarda datos del cuerpo de{ $slot3 }{$month}, conservados pero no leídos aquí, así que no se muestran en esta página.{ $slot4 }{$action}

journal.window.body.import.progress.running.days = { $total_quantity ->
    [one] { $done_quantity ->
        [one] todavía en curso: { $done_days } de { $total_days } día listo.
        [many] todavía en curso: { $done_days } de { $total_days } día listo.
       *[other] todavía en curso: { $done_days } de { $total_days } día listo.
    }
    [many] { $done_quantity ->
        [one] todavía en curso: { $done_days } de { $total_days } días listos.
        [many] todavía en curso: { $done_days } de { $total_days } días listos.
       *[other] todavía en curso: { $done_days } de { $total_days } días listos.
    }
   *[other] { $done_quantity ->
        [one] todavía en curso: { $done_days } de { $total_days } días listos.
        [many] todavía en curso: { $done_days } de { $total_days } días listos.
       *[other] todavía en curso: { $done_days } de { $total_days } días listos.
    }
}

journal.window.body.import.progress.running.empty = sigue en curso, y todavía no se guardó nada.

journal.window.body.import.progress.running.entries = { $quantity ->
    [one] sigue en curso: { $entry_count } entrada leída hasta ahora.
    [many] sigue en curso: { $entry_count } entradas leídas hasta ahora.
   *[other] sigue en curso: { $entry_count } entradas leídas hasta ahora.
}

journal.window.body.import.progress.stopped.days = { $total_quantity ->
    [one] { $done_quantity ->
        [one] se detuvo a medias: { $done_days } de { $total_days } día listo.
        [many] se detuvo a medias: { $done_days } de { $total_days } día listo.
       *[other] se detuvo a medias: { $done_days } de { $total_days } día listo.
    }
    [many] { $done_quantity ->
        [one] se detuvo a medias: { $done_days } de { $total_days } días listos.
        [many] se detuvo a medias: { $done_days } de { $total_days } días listos.
       *[other] se detuvo a medias: { $done_days } de { $total_days } días listos.
    }
   *[other] { $done_quantity ->
        [one] se detuvo a medias: { $done_days } de { $total_days } días listos.
        [many] se detuvo a medias: { $done_days } de { $total_days } días listos.
       *[other] se detuvo a medias: { $done_days } de { $total_days } días listos.
    }
}

journal.window.body.import.progress.stopped.empty = se detuvo a medias, y todavía no se guardó nada.

journal.window.body.import.progress.stopped.entries = { $quantity ->
    [one] se detuvo a medias: { $entry_count } entrada leída hasta ahora.
    [many] se detuvo a medias: { $entry_count } entradas leídas hasta ahora.
   *[other] se detuvo a medias: { $entry_count } entradas leídas hasta ahora.
}

journal.window.body.import.span.counted.days = { $quantity ->
    [one] { $first } a { $last }, { $count } día
    [many] { $first } a { $last }, { $count } días
   *[other] { $first } a { $last }, { $count } días
}

journal.window.body.import.span.counted.months = { $quantity ->
    [one] { $first } a { $last } ({ $count } mes)
    [many] { $first } a { $last } ({ $count } meses)
   *[other] { $first } a { $last } ({ $count } meses)
}

journal.window.body.import.span.month = {$month}{ $slot1 }(por mes)

journal.window.body.import.span.unknown = un intervalo desconocido

journal.window.body.link.label.body.overview = resumen del cuerpo →

journal.window.body.link.label.body.overview.variant2 = ← resumen del cuerpo

journal.window.body.link.label.body.overview.variant3 = resumen del cuerpo

journal.window.body.logged.who.by = tú, en{ $slot1 }{$source}

journal.window.body.logged.who.you = tú

journal.window.body.message.it.may.already.be.back.in = puede que ya haya vuelto a tu diario. recarga para revisar.

journal.window.body.message.it.may.already.be.erased.reload = puede que ya se haya borrado. recarga para revisar.

journal.window.body.message.it.may.already.be.in.the = puede que ya esté en la papelera. recarga para revisar.

journal.window.body.message.it.s.back.in.your.journal = volvió a tu diario.

journal.window.body.message.it.s.in.the.trash.you = está en la papelera. puedes volver a intentarlo desde ahí.

journal.window.body.message.it.s.still.in.the.trash = sigue en la papelera. puedes volver a intentarlo desde ahí.

journal.window.body.message.it.s.still.in.your.journal = sigue en tu diario.

journal.window.body.section.heading.counts = conteos

journal.window.body.section.heading.earlier.imports.kept.but.not.read = importaciones anteriores, conservadas pero no leídas aquí

journal.window.body.section.heading.every.day.held = cada día guardado

journal.window.body.section.heading.for.this.date = para esta fecha

journal.window.body.section.heading.imports = importaciones

journal.window.body.section.heading.logged = registrado

journal.window.body.section.heading.readings = lecturas

journal.window.body.section.heading.sources.held = fuentes guardadas

journal.window.body.section.heading.strava = Strava

journal.window.body.sleep.stage = fase de sueño{ $slot1 }{$stage}

journal.window.body.status.text.couldn.t.load.body.day = no se pudo cargar el día del cuerpo

journal.window.body.status.text.couldn.t.load.your.body.data = no se pudo cargar los datos de tu cuerpo

journal.window.body.strava.commute = viaje al trabajo

journal.window.body.strava.distance = {$distance}{ $slot1 }km

journal.window.body.strava.duration.hours = {$hours}{ $slot1 }h

journal.window.body.strava.duration.minutes = {$minutes}{ $slot1 }min

journal.window.body.strava.duration.time = {$hours}{ $slot1 }h{ $slot2 }{$minutes}{ $slot3 }minutos

journal.window.body.strava.elevation = desnivel positivo{ $slot1 }{$metres}{ $slot2 }m

journal.window.body.strava.energy = {$value}{ $slot1 }kcal

journal.window.body.strava.heart = frecuencia cardíaca prom.{ $slot1 }{$average}, máx.{ $slot2 }{$maximum}{ $slot3 }bpm

journal.window.body.strava.heart.average = frecuencia cardíaca prom.{ $slot1 }{$value}{ $slot2 }bpm

journal.window.body.strava.heart.maximum = frecuencia cardíaca máx.{ $slot1 }{$value}{ $slot2 }bpm

journal.window.body.strava.manual = ingresado a mano

journal.window.body.strava.moving.hours = en movimiento { $hours } h

journal.window.body.strava.moving.minutes = en movimiento { $minutes } min

journal.window.body.strava.moving.time = en movimiento { $hours } h { $minutes } min

journal.window.body.strava.power = potencia prom.{ $slot1 }{$average}{ $slot2 }W, ponderada{ $slot3 }{$weighted}{ $slot4 }W

journal.window.body.strava.power.average = potencia prom.{ $slot1 }{$value}{ $slot2 }W

journal.window.body.strava.power.weighted = potencia ponderada{ $slot1 }{$value}{ $slot2 }W

journal.window.body.strava.workout = entrenamiento

journal.window.body.title.couldn.t.erase.that.workout = no se pudo borrar ese entrenamiento

journal.window.body.title.couldn.t.move.that.to.the = no se pudo mover eso a la papelera

journal.window.body.title.couldn.t.recover.that.workout = no se pudo recuperar ese entrenamiento

journal.window.body.title.delete.this.workout = ¿eliminar este entrenamiento?

journal.window.body.title.erase.this.workout.now = ¿borrar este entrenamiento ahora?

journal.window.body.title.moved.to.the.trash = se movió a la papelera

journal.window.body.title.workout.erased.from.your.journal = entrenamiento borrado de tu diario

journal.window.body.title.workout.recovered = entrenamiento recuperado

journal.window.body.title.your.journal.didn.t.answer = tu diario no respondió

journal.window.body.visible.text.nothing.for.this.day = no hay nada para este día.

journal.window.body.visible.text.part.of.this.day.couldn.t = no se pudo leer parte de este día.

journal.window.body.visible.text.strava = · Strava

journal.window.body.visible.text.strava.variant2 = Strava

journal.window.body.window.explanation.your.body.brought.into.your.journal = tu cuerpo, traído a tu diario.

journal.window.body.window.state.text.loading.body.data = cargando datos del cuerpo…

journal.window.body.window.state.text.loading.body.day = cargando el día del cuerpo…

journal.window.body.workout.copy.again.importing.a.strava.download.again.brings = volver a importar una descarga de Strava trae de vuelta este entrenamiento. un día que ocultas permanece oculto cuando lo importas de nuevo.

journal.window.body.workout.copy.back.back = atrás

journal.window.body.workout.copy.cancel.cancel = cancelar

journal.window.body.workout.copy.erase.now.erase.now = borrar ahora

journal.window.body.workout.copy.move.to.trash.move.to.trash = mover a la papelera

journal.window.body.workout.copy.open.trash.open.trash = abrir la papelera

journal.window.body.workout.copy.reach.it.leaves.your.journal.now.and = sale de tu diario ahora y no se puede recuperar.

journal.window.body.workout.copy.reach.it.leaves.your.journal.now.and.variant2 = sale de tu diario ahora y espera en la papelera, y después se borra. puedes recuperarlo hasta entonces.

journal.window.body.workout.copy.recover.recover = recuperar

journal.window.body.workout.copy.stays.activities.summaries.and.entities.made.from = las actividades, los resúmenes y las entidades hechos a partir de esto en otra parte de tu diario no se borran y todavía pueden mencionarlo.

journal.window.body.workout.copy.stays.activities.summaries.and.entities.made.from.variant2 = las actividades, los resúmenes y las entidades hechos a partir de esto en otra parte de tu diario se quedan donde están y todavía pueden mencionarlo.

journal.window.body.workout.delete.name = eliminar { $workout_name }

journal.window.body.workout.strava.additional.count = { $quantity ->
    [one] más de Strava · { $count }
    [many] más de Strava · { $count }
   *[other] más de Strava · { $count }
}

journal.window.body.workout.strava.figures = cifras de Strava: { $figures }

journal.window.body.workout.trash.recovers.until.day = se borra el { $erase_day } a menos que lo recuperes.

journal.window.calendar.note.added = Se agregó un evento de calendario

journal.window.calendar.note.added.titled = Se agregó un evento de calendario: { $title }

journal.window.calendar.note.attendee.owner = { $name }, tú

journal.window.calendar.note.attendee.pair = { $attendees } y { $attendee }

journal.window.calendar.note.attendee.status = { $name } ({ $status })

journal.window.calendar.note.calendar = Calendario: { $calendar }

journal.window.calendar.note.calendar.account = Calendario: { $calendar } ({ $account })

journal.window.calendar.note.changed = Cambió un evento de calendario

journal.window.calendar.note.changed.titled = Cambió un evento de calendario: { $title }

journal.window.calendar.note.date = { $weekday }, { $month } { $day } de { $year }

journal.window.calendar.note.invited = Invitados: { $attendees }

journal.window.calendar.note.invited.more = { $remaining ->
    [one] Invitados: { $attendees }; y { $remaining } más
    [many] Invitados: { $attendees }; y { $remaining } más
   *[other] Invitados: { $attendees }; y { $remaining } más
}

journal.window.calendar.note.notes = Notas: { $notes }

journal.window.calendar.note.recorded = Registrado: { $date }, { $time } ({ $offset })

journal.window.calendar.note.removed = Se quitó un evento de calendario

journal.window.calendar.note.removed.titled = Se quitó un evento de calendario: { $title }

journal.window.calendar.note.repeats = Se repite: { $repeats }

journal.window.calendar.note.when.across.days = Cuándo: { $start_date }, { $start_time } a { $end_date }, { $end_time }

journal.window.calendar.note.when.across.days.zoned = Cuándo: { $start_date }, { $start_time } a { $end_date }, { $end_time } ({ $zone })

journal.window.calendar.note.when.all.day = Cuándo: { $date }, todo el día

journal.window.calendar.note.when.all.days = Cuándo: { $start_date } a { $end_date }, todo el día

journal.window.calendar.note.when.same.day = Cuándo: { $date }, { $start_time } a { $end_time }

journal.window.calendar.note.when.same.day.zoned = Cuándo: { $date }, { $start_time } a { $end_time } ({ $zone })

journal.window.calendar.note.when.time = Cuándo: { $date }, { $time }

journal.window.calendar.note.when.time.zoned = Cuándo: { $date }, { $time } ({ $zone })

journal.window.calendar.note.where = Dónde: { $location }

journal.window.chrome.connection.connecting = conectando…

journal.window.chrome.connection.delayed = algunas funciones pueden demorarse

journal.window.chrome.connection.reconnecting = se perdió la conexión. reconectando

journal.window.chrome.console.action.clear = limpiar

journal.window.chrome.console.action.close = cerrar

journal.window.chrome.console.action.send = enviar

journal.window.chrome.console.action.send.all = enviar a soporte

journal.window.chrome.console.action.show.details = mostrar detalles

journal.window.chrome.console.detail.location.label = ubicación

journal.window.chrome.console.detail.message.label = mensaje

journal.window.chrome.console.detail.stack.label = pila

journal.window.chrome.console.empty = no hay mensajes del sistema en esta sesión.

journal.window.chrome.console.heading = mensajes del sistema

journal.window.chrome.console.link.label = mensajes del sistema

journal.window.chrome.console.reporting.off = el envío de reportes está desactivado.

journal.window.chrome.console.summary.load.failed = no se pudo cargar parte de esta página.

journal.window.chrome.console.summary.request.failed = no se pudo terminar esa solicitud.

journal.window.chrome.console.summary.unexpected = algo en esta página no funcionó.

journal.window.chrome.console.tab.all = todo

journal.window.chrome.console.tab.errors = errores

journal.window.chrome.console.tab.info = información

journal.window.chrome.console.tab.warnings = advertencias

journal.window.chrome.day.zones.all = las horas de este día están en { $zones }

journal.window.chrome.day.zones.part = algunas horas de este día están en { $zones }

journal.window.chrome.log.read.failed = no se pudo leer ese registro. recarga para volver a intentarlo.

journal.window.chrome.reload.hint = recarga para volver a intentarlo.

journal.window.chrome.report.action.cancel = cancelar

journal.window.chrome.report.action.send = continuar al soporte

journal.window.chrome.report.app.label = app

journal.window.chrome.report.button.label = reportar esto

journal.window.chrome.report.default.subject = error de convey

journal.window.chrome.report.error.code.label = código de error

journal.window.chrome.report.privacy.note = nada sale de este diario hasta que envíes el formulario en el sitio de soporte.

journal.window.chrome.report.recent.label = líneas de error recientes

journal.window.chrome.report.route.label = ruta

journal.window.chrome.report.title = revisar este reporte

journal.window.chrome.retry = reintentar

journal.window.chrome.section.error = no se pudo cargar esta sección

journal.window.chrome.still.working = todavía trabajando…

journal.window.chrome.unknown.error = algo salió mal. recarga para volver a intentarlo.

journal.window.chrome.view.document.title = { $view ->
        [agents] agentes - journal
        [activities] actividades - journal
        [backup] copia de seguridad - journal
        [body] cuerpo - journal
        [curation] sugerencias - journal
        [entities] entidades - journal
        [health] salud - journal
        [home] inicio - journal
        [import] importación - journal
        [map] mapa - journal
        [more] más - journal
        [network] red - journal
        [search] búsqueda - journal
        [settings] configuración - journal
        [sources] fuentes - journal
        [speakers] hablantes - journal
        [stats] estadísticas - journal
        [support] soporte - journal
        [thinking] razonamiento - journal
        [transcripts] transcripciones - journal
        [year] año - journal
       *[other] { $fallback_label } - journal
    }

journal.window.chrome.zone.time = hora de { $label }

journal.window.content.action.heading = ### { $action } por { $actor }

journal.window.content.action.parameters = **Parámetros:**

journal.window.content.action.source = **Fuente:** { $source }

journal.window.content.action.talent = **Talento:** [{ $id }](/app/thinking/#runs/run/{ $id })

journal.window.content.action.time = **Hora:** { $time }

journal.window.content.action.unknown = desconocido

journal.window.content.activity.field.activity = - Actividad: { $value }

journal.window.content.activity.field.day = - Día: { $value }

journal.window.content.activity.field.description = - Descripción: { $value }

journal.window.content.activity.field.details = - Detalles: { $value }

journal.window.content.activity.field.facet = - Faceta: { $value }

journal.window.content.activity.field.kind = - Tipo: { $value }

journal.window.content.activity.field.level = - Nivel del último segmento en uso: { $value }

journal.window.content.activity.field.time = - Tiempo: { $value }

journal.window.content.activity.hidden = - Oculto: sí

journal.window.content.activity.omitted = No todo lo guardado con esta actividad cupo en esta entrada. El registro completo de la actividad lo tiene todo.

journal.window.content.activity.role.attendee = - Asistentes: { $names }

journal.window.content.activity.role.heard = - En conversación: { $names }

journal.window.content.activity.role.mentioned = - Mencionados: { $names }

journal.window.content.activity.role.other = - Participación: { $names }

journal.window.content.activity.role.visible = - En pantalla: { $names }

journal.window.content.activity.saved.relation = Relación guardada: { $value }

journal.window.content.activity.topics = Temas: { $topics }

journal.window.content.activity.untitled = Actividad sin título

journal.window.content.body.attribution = como { $source } lo registró

journal.window.content.body.attribution.from = como { $source } lo registró, desde { $by }

journal.window.content.body.attribution.values = { $attribution }: { $values }

journal.window.content.body.heading = ## { $source } { $kind }: { $label }

journal.window.content.body.kind.default = experiencia

journal.window.content.body.span = { $start } a { $end }

journal.window.content.browser.app.paused = app de solstone en pausa

journal.window.content.browser.context = contexto: { $details }

journal.window.content.browser.duration.d = { $days } d

journal.window.content.browser.duration.dh = { $days } d { $hours } h

journal.window.content.browser.duration.h = { $hours } h

journal.window.content.browser.duration.hm = { $hours } h { $minutes } min

journal.window.content.browser.duration.m = { $minutes } min

journal.window.content.browser.duration.s = { $seconds } s

journal.window.content.browser.language = idioma { $lang }

journal.window.content.browser.media.playing = contenido multimedia en reproducción

journal.window.content.browser.media.playing.muted = contenido multimedia en reproducción, silenciado

journal.window.content.browser.page.default = Página del navegador

journal.window.content.browser.tab.age = pestaña abierta al menos { $duration }

journal.window.content.browser.tab.background = pestaña en segundo plano

journal.window.content.browser.tab.front = pestaña al frente

journal.window.content.browser.tab.front.focused = pestaña al frente, ventana enfocada

journal.window.content.browser.tab.front.unfocused = pestaña al frente, ventana sin enfocar

journal.window.content.chat.speaker.assistant = Asistente

journal.window.content.chat.speaker.human = Humano

journal.window.content.chat.speaker.unknown = Desconocido

journal.window.content.chat.speaker.you = Tú

journal.window.content.chat.turn = **{ $speaker }**: { $text }

journal.window.content.desktop.activity.field.app = app: { $value }

journal.window.content.desktop.activity.field.app.id = id de la app: { $value }

journal.window.content.desktop.activity.field.capability = capacidad: { $value }

journal.window.content.desktop.activity.field.cwd = directorio de trabajo: { $value }

journal.window.content.desktop.activity.field.end = fin en ms: { $value }

journal.window.content.desktop.activity.field.exe = ejecutable: { $value }

journal.window.content.desktop.activity.field.kind = tipo: { $value }

journal.window.content.desktop.activity.field.method = método: { $value }

journal.window.content.desktop.activity.field.pane.title = título del panel: { $value }

journal.window.content.desktop.activity.field.path = ruta: { $value }

journal.window.content.desktop.activity.field.session = sesión: { $value }

journal.window.content.desktop.activity.field.start = inicio en ms: { $value }

journal.window.content.desktop.activity.field.state = estado: { $value }

journal.window.content.desktop.activity.field.status = estado: { $value }

journal.window.content.desktop.activity.field.stream = flujo: { $value }

journal.window.content.desktop.activity.field.title = título: { $value }

journal.window.content.desktop.activity.field.url = url: { $value }

journal.window.content.desktop.activity.field.window = ventana: { $value }

journal.window.content.desktop.activity.heading = actividad de escritorio

journal.window.content.document.absent = No especificado en este documento

journal.window.content.document.asset.unnamed = Bien sin nombre

journal.window.content.document.detail.applies = Aplica a: { $value }

journal.window.content.document.detail.timing = Plazo: { $value }

journal.window.content.document.party.unnamed = Parte sin nombre

journal.window.content.document.section.assets = Bienes y propiedad

journal.window.content.document.section.conditions = Condiciones y desencadenantes

journal.window.content.document.section.dates = Fechas importantes

journal.window.content.document.section.overview = Descripción general

journal.window.content.document.section.parties = Partes y roles

journal.window.content.document.section.provisions = Disposiciones clave

journal.window.content.document.section.summary = Resumen

journal.window.content.entity.aka = **También se conoce como:** { $items }

journal.window.content.entity.description.none = *(No hay descripción disponible)*

journal.window.content.entity.name.unnamed = Sin nombre

journal.window.content.entity.tags = **Etiquetas:** { $items }

journal.window.content.entity.type.unknown = Desconocido

journal.window.content.event.created = **Creado el:** { $date }

journal.window.content.event.heading.occurred = ### { $kind }: { $title }

journal.window.content.event.heading.planned = ### Planificado: { $kind }: { $title }

journal.window.content.event.participants.occurred = **Participantes:** { $names }

journal.window.content.event.participants.planned = **Participantes esperados:** { $names }

journal.window.content.event.time.occurred = **Cuándo ocurrió:** { $start }

journal.window.content.event.time.occurred.range = **Cuándo ocurrió:** { $start } - { $end }

journal.window.content.event.time.scheduled = **Cuándo está programado:** { $start }

journal.window.content.event.time.scheduled.range = **Cuándo está programado:** { $start } - { $end }

journal.window.content.event.type.default = Evento

journal.window.content.import.attendees = Asistentes: { $attendees }

journal.window.content.import.book.by = ## { $book } por { $author }

journal.window.content.import.book.unknown = Libro desconocido

journal.window.content.import.duration.min = { $minutes } min

journal.window.content.import.event.untitled = Evento sin título

journal.window.content.import.links = Enlaces: { $links }

journal.window.content.import.loc = Ubicación { $location }

journal.window.content.import.location = Ubicación: { $location }

journal.window.content.import.note.line = Nota: { $content }

journal.window.content.import.note.untitled = Nota sin título

journal.window.content.import.page = Página { $page }

journal.window.content.import.tags = Etiquetas: { $tags }

journal.window.content.objects.class.bird = pájaro

journal.window.content.objects.class.bottle = botella

journal.window.content.objects.class.bowl = tazón

journal.window.content.objects.class.car = auto

journal.window.content.objects.class.cell.phone = teléfono celular

journal.window.content.objects.class.cup = taza

journal.window.content.objects.class.laptop = portátil

journal.window.content.objects.class.person = persona

journal.window.content.objects.class.tie = corbata

journal.window.content.objects.class.truck = camión

journal.window.content.objects.class.tv = televisor

journal.window.content.objects.count = { $name } ×{ $count }

journal.window.content.objects.line = **Objetos:** { $summary }

journal.window.content.observation.observed = - { $content } (observado: { $day })

journal.window.content.possible.name = nombre posible: { $name }

journal.window.content.protected.video.body = Una ventana de { $service } estuvo en pantalla alrededor de { $duration } mientras la recepción (pantalla y audio) estaba en pausa.

journal.window.content.protected.video.body.app = Una ventana de { $service } estuvo en pantalla en { $app } alrededor de { $duration } mientras la recepción (pantalla y audio) estaba en pausa.

journal.window.content.protected.video.body.app.title = Una ventana de { $service } estuvo en pantalla en { $app }, titulada "{ $title }", alrededor de { $duration } mientras la recepción (pantalla y audio) estaba en pausa.

journal.window.content.protected.video.body.title = Una ventana de { $service } estuvo en pantalla, titulada "{ $title }", alrededor de { $duration } mientras la recepción (pantalla y audio) estaba en pausa.

journal.window.content.protected.video.heading = ## Video protegido: { $service }

journal.window.content.protected.video.minutes = { $quantity ->
    [one] 1 minuto
    [many] { $quantity } minutos
   *[other] { $quantity } minutos
}

journal.window.content.raw.screen.category = **Categoría:** { $category }

journal.window.content.raw.screen.meeting = **Reunión** ({ $platform })

journal.window.content.raw.screen.participants = **Participantes:**

journal.window.content.raw.screen.share = **Pantalla compartida:**

journal.window.content.raw.screen.share.by = **Pantalla compartida por { $presenter }:**

journal.window.content.raw.screen.unknown = desconocido

journal.window.content.raw.screen.unknown.title = Desconocido

journal.window.content.screen.entities.heading = ## Entidades

journal.window.content.screen.entities.none = No especificado

journal.window.content.screen.entity.default = Entidad

journal.window.content.workout.calories = { $kcal } kcal

journal.window.content.workout.distance = { $km } km

journal.window.content.workout.duration.h = { $hours } h

journal.window.content.workout.duration.hm = { $hours } h y { $minutes } min

journal.window.content.workout.duration.m = { $minutes } min

journal.window.content.workout.elapsed = { $duration } transcurrido

journal.window.content.workout.elevation = desnivel positivo { $m } m

journal.window.content.workout.flag.commute = viaje al trabajo

journal.window.content.workout.flag.manual = ingresado a mano

journal.window.content.workout.heading = ## Entrenamiento de Strava: { $name }

journal.window.content.workout.hr.avg = frecuencia cardíaca promedio { $avg } bpm

journal.window.content.workout.hr.avg.max = frecuencia cardíaca promedio { $avg } bpm, máxima { $max } bpm

journal.window.content.workout.hr.max = frecuencia cardíaca máxima { $max } bpm

journal.window.content.workout.more = más de Strava:

journal.window.content.workout.moving = { $duration } en movimiento

journal.window.content.workout.name.default = entrenamiento

journal.window.content.workout.power.avg = potencia promedio { $avg } W

journal.window.content.workout.power.avg.weighted = potencia promedio { $avg } W, ponderada { $weighted } W

journal.window.content.workout.power.weighted = potencia ponderada { $weighted } W

journal.window.content.workout.recorded = como lo registró Strava: { $details }

journal.window.content.workout.started = empezó { $time }

journal.window.home.accessible.name.days.of.the.week = días de la semana

journal.window.home.accessible.name.originals.pages = páginas de originales

journal.window.home.accessible.name.which.day = qué día

journal.window.home.account.tab.today = hoy

journal.window.home.account.tab.yesterday = ayer

journal.window.home.action.all.of.todays.activities = todas las actividades de hoy →

journal.window.home.action.check.status = revisar el estado

journal.window.home.action.copy = copiar

journal.window.home.action.how.this.was.written = cómo se escribió esto →

journal.window.home.action.open.it = abrirlo →

journal.window.home.action.open.your.week = abrir tu semana →

journal.window.home.action.see.health = ver salud →

journal.window.home.action.see.the.run = ver la ejecución →

journal.window.home.action.undo = deshacer

journal.window.home.action.what.it.fixes = qué corrige ↗

journal.window.home.activity.source.browser = de tu navegador

journal.window.home.activity.source.browser.named = de tu navegador ({ $device })

journal.window.home.activity.source.computer = de tu computadora

journal.window.home.activity.source.computer.named = de tu computadora ({ $device })

journal.window.home.activity.source.ipad = de tu ipad

journal.window.home.activity.source.ipad.named = de tu ipad ({ $device })

journal.window.home.activity.source.iphone = de tu iphone

journal.window.home.activity.source.iphone.named = de tu iphone ({ $device })

journal.window.home.activity.source.mac = de tu mac

journal.window.home.activity.source.mac.named = de tu mac ({ $device })

journal.window.home.activity.source.phone = de tu teléfono

journal.window.home.activity.source.phone.named = de tu teléfono ({ $device })

journal.window.home.activity.source.phrase = de tu{ $slot1 }{ $kind }

journal.window.home.activity.source.phrase.named = de tu{ $slot1 }{ $kind }{ $slot2 }({ $device })

journal.window.home.activity.source.tablet = de tu tableta

journal.window.home.activity.source.tablet.named = de tu tableta ({ $device })

journal.window.home.activity.source.terminal = de tu terminal

journal.window.home.activity.source.terminal.named = de tu terminal ({ $device })

journal.window.home.activity.source.watch = de tu reloj

journal.window.home.activity.source.watch.named = de tu reloj ({ $device })

journal.window.home.activity.title.fallback = actividad sin título

journal.window.home.attention.agent.errors.count = { $quantity ->
    [one] { $count } error de agente hoy. pregunta qué pasó
    [many] { $count } errores de agente hoy. pregunta qué pasó
   *[other] { $count } errores de agente hoy. pregunta qué pasó
}

journal.window.home.attention.heading = esperándote

journal.window.home.attention.import.complete = importación completa.

journal.window.home.body.account.mindfulness = atención plena

journal.window.home.body.account.sleep = sueño

journal.window.home.body.activity.line = { $arrangement ->
       *[untimed-no-distance-no-source-intact] { $activity }.
        [untimed-no-distance-no-source-part-removed] { $activity }, con una parte eliminada.
        [untimed-no-distance-logged-intact] { $activity }, que registraste.
        [untimed-no-distance-logged-part-removed] { $activity }, que registraste, con una parte eliminada.
        [untimed-no-distance-recorded-intact] { $activity }, de { $source_names }.
        [untimed-no-distance-recorded-part-removed] { $activity }, de { $source_names }, con una parte eliminada.
        [untimed-distance-no-source-intact] { $activity }, { $distance }.
        [untimed-distance-no-source-part-removed] { $activity }, { $distance }, con una parte eliminada.
        [untimed-distance-logged-intact] { $activity }, { $distance }, que registraste.
        [untimed-distance-logged-part-removed] { $activity }, { $distance }, que registraste, con una parte eliminada.
        [untimed-distance-recorded-intact] { $activity }, { $distance }, de { $source_names }.
        [untimed-distance-recorded-part-removed] { $activity }, { $distance }, de { $source_names }, con una parte eliminada.
        [start-no-distance-no-source-intact] { $activity }, desde { $started_at }.
        [start-no-distance-no-source-part-removed] { $activity }, desde { $started_at }, con una parte eliminada.
        [start-no-distance-logged-intact] { $activity }, desde { $started_at }, que registraste.
        [start-no-distance-logged-part-removed] { $activity }, desde { $started_at }, que registraste, con una parte eliminada.
        [start-no-distance-recorded-intact] { $activity }, desde { $started_at }, de { $source_names }.
        [start-no-distance-recorded-part-removed] { $activity }, desde { $started_at }, de { $source_names }, con una parte eliminada.
        [start-distance-no-source-intact] { $activity }, desde { $started_at }, { $distance }.
        [start-distance-no-source-part-removed] { $activity }, desde { $started_at }, { $distance }, con una parte eliminada.
        [start-distance-logged-intact] { $activity }, desde { $started_at }, { $distance }, que registraste.
        [start-distance-logged-part-removed] { $activity }, desde { $started_at }, { $distance }, que registraste, con una parte eliminada.
        [start-distance-recorded-intact] { $activity }, desde { $started_at }, { $distance }, de { $source_names }.
        [start-distance-recorded-part-removed] { $activity }, desde { $started_at }, { $distance }, de { $source_names }, con una parte eliminada.
        [range-no-distance-no-source-intact] { $activity }, de { $started_at } a { $ended_at }.
        [range-no-distance-no-source-part-removed] { $activity }, de { $started_at } a { $ended_at }, con una parte eliminada.
        [range-no-distance-logged-intact] { $activity }, de { $started_at } a { $ended_at }, que registraste.
        [range-no-distance-logged-part-removed] { $activity }, de { $started_at } a { $ended_at }, que registraste, con una parte eliminada.
        [range-no-distance-recorded-intact] { $activity }, de { $started_at } a { $ended_at }, de { $source_names }.
        [range-no-distance-recorded-part-removed] { $activity }, de { $started_at } a { $ended_at }, de { $source_names }, con una parte eliminada.
        [range-distance-no-source-intact] { $activity }, de { $started_at } a { $ended_at }, { $distance }.
        [range-distance-no-source-part-removed] { $activity }, de { $started_at } a { $ended_at }, { $distance }, con una parte eliminada.
        [range-distance-logged-intact] { $activity }, de { $started_at } a { $ended_at }, { $distance }, que registraste.
        [range-distance-logged-part-removed] { $activity }, de { $started_at } a { $ended_at }, { $distance }, que registraste, con una parte eliminada.
        [range-distance-recorded-intact] { $activity }, de { $started_at } a { $ended_at }, { $distance }, de { $source_names }.
        [range-distance-recorded-part-removed] { $activity }, de { $started_at } a { $ended_at }, { $distance }, de { $source_names }, con una parte eliminada.
    }

journal.window.home.body.sources.and = { $start }{ $slot1 }y{ $slot2 }{ $last }

journal.window.home.body.time.with.day = { $time }{ $slot1 }el{ $slot2 }{ $day }

journal.window.home.briefing.expected.time = ayer suele estar escrito para las 10 a. m.

journal.window.home.briefing.late.hour.count = { $quantity ->
    [one] lleva { $hours } h de retraso.
    [many] lleva { $hours } h de retraso.
   *[other] lleva { $hours } h de retraso.
}

journal.window.home.briefing.written.timestamp = escrito a las { $written_at }

journal.window.home.button.label.show.all = mostrar todo →

journal.window.home.button.label.try.again = Reintentar

journal.window.home.connections.events.count = { $quantity ->
    [one] 1 evento
    [many] { $count } eventos
   *[other] { $count } eventos
}

journal.window.home.connections.evidence.transcription = se transcribió como "{ $label }"

journal.window.home.connections.horizon.day = las conexiones de tiempo en el mismo lugar se remontan solo hasta { $day }.

journal.window.home.connections.horizon.earlier.days = { $quantity ->
    [one] un día anterior sigue en tu diario; esas conexiones todavía no se han construido.
    [many] los { $earlier_days } días anteriores siguen en tu diario; esas conexiones todavía no se han construido.
   *[other] los { $earlier_days } días anteriores siguen en tu diario; esas conexiones todavía no se han construido.
}

journal.window.home.connections.mentions.count = { $quantity ->
    [one] 1 nombre mencionado en tu diario
    [many] { $count } nombres mencionados en tu diario
   *[other] { $count } nombres mencionados en tu diario
}

journal.window.home.connections.moments.count = { $quantity ->
    [one] 1 momento
    [many] { $count } momentos
   *[other] { $count } momentos
}

journal.window.home.connections.moments.recent.count = { $quantity ->
    [one] 1 momento · más recientemente { $day }
    [many] { $count } momentos · más recientemente { $day }
   *[other] { $count } momentos · más recientemente { $day }
}

journal.window.home.device.last.added.day = { $quantity ->
    [one] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } día.
    [many] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } días.
   *[other] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } días.
}

journal.window.home.device.last.added.hour = { $quantity ->
    [one] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } hora.
    [many] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } horas.
   *[other] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } horas.
}

journal.window.home.device.last.added.minute = { $quantity ->
    [one] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } minuto.
    [many] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } minutos.
   *[other] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } minutos.
}

journal.window.home.device.last.added.month = { $quantity ->
    [one] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } mes.
    [many] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } meses.
   *[other] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } meses.
}

journal.window.home.device.last.added.second = { $quantity ->
    [one] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } segundo.
    [many] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } segundos.
   *[other] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } segundos.
}

journal.window.home.device.last.added.week = { $quantity ->
    [one] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } semana.
    [many] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } semanas.
   *[other] la app de solstone en { $name } agregó a tu diario por última vez hace { $count } semanas.
}

journal.window.home.device.paired = la app de solstone en{ $slot1 }{ $name }{ $slot2 }está vinculada.

journal.window.home.device.paired.awaiting = la app de solstone en{ $slot1 }{ $name }{ $slot2 }está vinculada. lo que compartes con ella llega a tu diario más o menos cada 5 minutos.

journal.window.home.disclosure.label.review.originals = revisar los originales

journal.window.home.disclosure.label.what.this.summary.covers = qué cubre este resumen

journal.window.home.download.speech.progress = descargando el modelo de habla · { $downloaded_gb } GB de { $total_gb } GB

journal.window.home.download.thinking.progress = descargando el modelo local · { $downloaded_gb } GB de { $total_gb } GB

journal.window.home.first.segment.arrived = tu primer segmento, desde{ $slot1 }{ $time }, llegó.

journal.window.home.first.segment.pending = tu primer segmento, desde{ $slot1 }{ $time }, llegó. el razonamiento todavía no se puso al día con él.

journal.window.home.first.segment.read = tu primer segmento, desde{ $slot1 }{ $time }, llegó, y el razonamiento lo leyó.

journal.window.home.health.age.just.now = justo ahora

journal.window.home.health.attention.count = { $quantity ->
    [one] 1 cosa necesita tu atención
    [many] { $count } cosas necesitan tu atención
   *[other] { $count } cosas necesitan tu atención
}

journal.window.home.health.awaiting.first.headline = la app de solstone en uno de tus dispositivos todavía no ha agregado nada a tu diario.

journal.window.home.health.backlog.day.needs.help = un día del diario necesita una mano.

journal.window.home.health.device.age = { $device } ({ $age })

journal.window.home.health.device.setup = configurar uno →

journal.window.home.health.device.sources = { $device } ({ $sources })

journal.window.home.health.devices.view = ver dispositivos →

journal.window.home.health.empty.registry.headline = todavía no hay dispositivos ejecutando la app de solstone. configura uno para empezar tu diario.

journal.window.home.health.no.eligible.headline = ninguno de tus dispositivos tiene la app de solstone configurada para agregar a tu diario ahora mismo.

journal.window.home.health.not.yet.active.headline = la app de solstone está agregando a tu diario.

journal.window.home.health.quiet.devices = { $quantity ->
    [one] { $count } dispositivo no ha agregado nada a tu diario recientemente: { $devices }.
    [many] { $count } dispositivos no han agregado nada a tu diario recientemente: { $devices }.
   *[other] { $count } dispositivos no han agregado nada a tu diario recientemente: { $devices }.
}

journal.window.home.health.quiet.elapsed.day = { $quantity ->
    [one] { $device } no ha agregado nada a tu diario desde hace { $count } día.
    [many] { $device } no ha agregado nada a tu diario desde hace { $count } días.
   *[other] { $device } no ha agregado nada a tu diario desde hace { $count } días.
}

journal.window.home.health.quiet.elapsed.hour = { $quantity ->
    [one] { $device } no ha agregado nada a tu diario desde hace { $count } hora.
    [many] { $device } no ha agregado nada a tu diario desde hace { $count } horas.
   *[other] { $device } no ha agregado nada a tu diario desde hace { $count } horas.
}

journal.window.home.health.quiet.elapsed.just.now = { $device } agregó a tu diario por última vez justo ahora.

journal.window.home.health.quiet.elapsed.minute = { $quantity ->
    [one] { $device } no ha agregado nada a tu diario desde hace { $count } minuto.
    [many] { $device } no ha agregado nada a tu diario desde hace { $count } minutos.
   *[other] { $device } no ha agregado nada a tu diario desde hace { $count } minutos.
}

journal.window.home.health.quiet.elapsed.month = { $quantity ->
    [one] { $device } no ha agregado nada a tu diario desde hace { $count } mes.
    [many] { $device } no ha agregado nada a tu diario desde hace { $count } meses.
   *[other] { $device } no ha agregado nada a tu diario desde hace { $count } meses.
}

journal.window.home.health.quiet.elapsed.second = { $quantity ->
    [one] { $device } no ha agregado nada a tu diario desde hace { $count } segundo.
    [many] { $device } no ha agregado nada a tu diario desde hace { $count } segundos.
   *[other] { $device } no ha agregado nada a tu diario desde hace { $count } segundos.
}

journal.window.home.health.quiet.elapsed.week = { $quantity ->
    [one] { $device } no ha agregado nada a tu diario desde hace { $count } semana.
    [many] { $device } no ha agregado nada a tu diario desde hace { $count } semanas.
   *[other] { $device } no ha agregado nada a tu diario desde hace { $count } semanas.
}

journal.window.home.health.quiet.one.unknown = { $device }{ $slot1 }no ha agregado nada a tu diario recientemente.

journal.window.home.health.ready = todo funciona

journal.window.home.health.residue.headline = nada necesita tu atención ahora mismo.

journal.window.home.health.unavailable.headline = el estado de tus dispositivos no está claro ahora mismo.

journal.window.home.health.unnamed.idle.issue = la app de solstone en uno de tus dispositivos no ha agregado nada a tu diario recientemente.

journal.window.home.health.upload.devices = { $quantity ->
    [one] tu diario no pudo aceptar subidas desde { $devices }.
    [many] tu diario no pudo aceptar subidas desde { $count } dispositivos: { $devices }.
   *[other] tu diario no pudo aceptar subidas desde { $count } dispositivos: { $devices }.
}

journal.window.home.held.activity.safety.check = el procesamiento de la actividad está en espera para una revisión de seguridad.

journal.window.home.held.all.safety.check = todo el trabajo en cola está en espera para una revisión de seguridad.

journal.window.home.held.daily.safety.check = el procesamiento diario está en espera para una revisión de seguridad.

journal.window.home.held.flush.safety.check = la finalización de las actividades está en espera para una revisión de seguridad.

journal.window.home.held.queued.safety.check = el procesamiento en cola está en espera para una revisión de seguridad.

journal.window.home.held.search.safety.check = las actualizaciones de la búsqueda están en espera para una revisión de seguridad.

journal.window.home.held.segment.safety.check = el procesamiento del segmento está en espera para una revisión de seguridad.

journal.window.home.held.weekly.safety.check = la reflexión semanal está en espera para una revisión de seguridad.

journal.window.home.link.label.all.connections = todas las conexiones →

journal.window.home.link.label.cancel.on.the.thinking.page = cancelar en la página de razonamiento →

journal.window.home.link.label.check.system.health = revisar el estado del sistema →

journal.window.home.link.label.health = salud →

journal.window.home.link.label.name.voices.in.speakers = nombrar voces en hablantes →

journal.window.home.link.label.other.agents.and.pairing.codes = otros agentes y códigos de vinculación →

journal.window.home.link.label.pair.a.device = vincular un dispositivo →

journal.window.home.link.label.try.again.on.the.thinking.page = reintentar en la página de razonamiento →

journal.window.home.narrative.coverage.batch.counts = { $segment_quantity ->
    [one] a partir de { $processed_segments } de { $batch_segments } segmento procesado y { $processed_activities } de { $batch_activities } { $activity_quantity ->
        [one] entrada de actividad
        [many] entradas de actividad
       *[other] entradas de actividad
    } en este lote.
    [many] a partir de { $processed_segments } de { $batch_segments } segmentos procesados y { $processed_activities } de { $batch_activities } { $activity_quantity ->
        [one] entrada de actividad
        [many] entradas de actividad
       *[other] entradas de actividad
    } en este lote.
   *[other] a partir de { $processed_segments } de { $batch_segments } segmentos procesados y { $processed_activities } de { $batch_activities } { $activity_quantity ->
        [one] entrada de actividad
        [many] entradas de actividad
       *[other] entradas de actividad
    } en este lote.
}

journal.window.home.narrative.coverage.clock = esto es tiempo de procesamiento, no cuándo ocurrió la actividad.

journal.window.home.narrative.coverage.overlap = las entradas pueden superponerse; esto no es un resumen de todo el día.

journal.window.home.narrative.coverage.started = la ventana de procesamiento empieza a las { $start_time }.

journal.window.home.narrative.updated.timestamp = actualizado a las { $updated_at }

journal.window.home.needs.device.one = { $name }{ $slot1 }no está llegando a tu diario.

journal.window.home.needs.devices.named = { $quantity ->
    [one] { $count } dispositivo no está llegando a tu diario: { $names }.
    [many] { $count } dispositivos no están llegando a tu diario: { $names }.
   *[other] { $count } dispositivos no están llegando a tu diario: { $names }.
}

journal.window.home.needs.devices.unnamed = { $quantity ->
    [one] { $count } dispositivo no está llegando a tu diario.
    [many] { $count } dispositivos no están llegando a tu diario.
   *[other] { $count } dispositivos no están llegando a tu diario.
}

journal.window.home.needs.devices.unnamed.count = { $quantity ->
    [one] { $count } dispositivo sin nombre
    [many] { $count } dispositivos sin nombre
   *[other] { $count } dispositivos sin nombre
}

journal.window.home.needs.sources.failing = la app de solstone en uno de tus dispositivos tiene problemas para agregar{ $slot1 }{ $sources }{ $slot2 }a tu diario.

journal.window.home.needs.voice.candidate.ready = puede que tu diario haya encontrado tu voz. reproduce algunas muestras y di si eres tú.

journal.window.home.needs.voice.damaged = no se puede leer tu voz guardada. quítala en la página de hablantes y luego enséñala de nuevo.

journal.window.home.needs.voice.declined.template = tu voz todavía no está configurada. tu diario vuelve a buscarla después de{ $slot1 }{ $date }, o puedes enseñarla ahora.

journal.window.home.needs.voice.needs.more.audio = tu voz todavía no está configurada. tu diario la busca cada día, o puedes enseñarla ahora.

journal.window.home.needs.voice.next.look.date = { $weekday }{ $slot1 }{ $month }{ $slot2 }{ $day }

journal.window.home.needs.voice.too.short.or.mixed = tu voz todavía no está configurada. hasta ahora lo que dices es sobre todo breve o está mezclado con otras voces, así que enseñarla ayuda más.

journal.window.home.needs.voice.unusable = no se puede configurar tu voz con lo que hay. enséñala en la página de hablantes.

journal.window.home.night.briefing.not.written = ayer no se redactó.

journal.window.home.night.gap.additional = { $quantity ->
    [one] …y { $count } más no terminó.
    [many] …y { $count } más no terminaron.
   *[other] …y { $count } más no terminaron.
}

journal.window.home.night.gap.runs.failed = { $quantity ->
    [one] la ejecución de { $label } no terminó.
    [many] { $count } ejecuciones de { $label } no terminaron.
   *[other] { $count } ejecuciones de { $label } no terminaron.
}

journal.window.home.night.gap.runs.failed.briefing = { $quantity ->
    [one] la ejecución del resumen diario no terminó.
    [many] { $count } ejecuciones del resumen diario no terminaron.
   *[other] { $count } ejecuciones del resumen diario no terminaron.
}

journal.window.home.night.gap.runs.failed.documents = { $quantity ->
    [one] la ejecución del documento no terminó.
    [many] { $count } ejecuciones del documento no terminaron.
   *[other] { $count } ejecuciones del documento no terminaron.
}

journal.window.home.night.gap.runs.lost = { $quantity ->
    [one] la ejecución de { $label } no se pudo iniciar.
    [many] { $count } ejecuciones de { $label } no se pudieron iniciar.
   *[other] { $count } ejecuciones de { $label } no se pudieron iniciar.
}

journal.window.home.night.gap.runs.lost.briefing = { $quantity ->
    [one] la ejecución del resumen diario no se pudo iniciar.
    [many] { $count } ejecuciones del resumen diario no se pudieron iniciar.
   *[other] { $count } ejecuciones del resumen diario no se pudieron iniciar.
}

journal.window.home.night.gap.runs.lost.documents = { $quantity ->
    [one] la ejecución del documento no se pudo iniciar.
    [many] { $count } ejecuciones del documento no se pudieron iniciar.
   *[other] { $count } ejecuciones del documento no se pudieron iniciar.
}

journal.window.home.night.notes.not.written = no se escribieron todas las notas de ayer.

journal.window.home.night.processing.did.not.finish = parte del procesamiento de la noche no terminó.

journal.window.home.night.review.did.not.finish = la revisión de la noche no terminó.

journal.window.home.night.status.unavailable = el estado del procesamiento de ayer no está disponible.

journal.window.home.night.work.did.not.finish = parte del trabajo de la noche no terminó.

journal.window.home.originals.bulk.limit = elige hasta { $selection_limit } elementos a la vez. no se eliminó nada.

journal.window.home.originals.bulk.selection.count = { $quantity ->
    [one] 1 seleccionado
    [many] { $selected_count } seleccionados
   *[other] { $selected_count } seleccionados
}

journal.window.home.originals.bulk.selection.over.cap = { $selected_count } seleccionados. una acción abarca hasta { $selection_limit }.

journal.window.home.originals.card.page.total = { $quantity ->
    [one] en esta página: 1 original · { $size }
    [many] en esta página: { $original_count } originales · { $size }
   *[other] en esta página: { $original_count } originales · { $size }
}

journal.window.home.originals.confirm.bulk.delete.first = esto elimina { $original_count } originales.

journal.window.home.originals.confirm.bulk.delete.third = no va nada más con ellos.

journal.window.home.originals.confirm.bulk.trash.third = esperan en la papelera, luego la papelera los borra y su espacio vuelve.

journal.window.home.originals.confirm.bulk.trash.until.erase.first = esto mueve { $original_count } originales a la papelera.

journal.window.home.originals.confirm.bulk.trash.until.erase.fourth = puedes recuperarlos hasta entonces.

journal.window.home.originals.confirm.bulk.trash.until.erase.third = { $days_quantity ->
    [one] la papelera los borra después de 1 día, y entonces vuelve su espacio.
    [many] la papelera los borra después de { $retention_days } días, y entonces vuelve su espacio.
   *[other] la papelera los borra después de { $retention_days } días, y entonces vuelve su espacio.
}

journal.window.home.originals.confirm.delete.action = { $quantity ->
    [one] eliminarlo
    [many] eliminarlos
   *[other] eliminarlos
}

journal.window.home.originals.confirm.delete.default.stream.first = { $quantity ->
    [one] esto elimina 1 original de { $date_and_time }.
    [many] esto elimina { $original_count } originales de { $date_and_time }.
   *[other] esto elimina { $original_count } originales de { $date_and_time }.
}

journal.window.home.originals.confirm.delete.heading = { $quantity ->
    [one] ¿eliminar este original?
    [many] ¿eliminar estos originales?
   *[other] ¿eliminar estos originales?
}

journal.window.home.originals.confirm.delete.named.stream.first = { $quantity ->
    [one] esto elimina 1 original de { $date_and_time } · { $stream_name }.
    [many] esto elimina { $original_count } originales de { $date_and_time } · { $stream_name }.
   *[other] esto elimina { $original_count } originales de { $date_and_time } · { $stream_name }.
}

journal.window.home.originals.confirm.delete.named.stream.fourth = no se puede deshacer.

journal.window.home.originals.confirm.delete.named.stream.second = tu copia de seguridad tiene una copia que no se puede revisar desde aquí.

journal.window.home.originals.confirm.delete.named.stream.third = { $quantity ->
    [one] no va nada más con él.
    [many] no va nada más con ellos.
   *[other] no va nada más con ellos.
}

journal.window.home.originals.confirm.mixed.delete.first = { $quantity ->
    [one] el que se elimina fue copiado fuera de tu dispositivo por tu copia de seguridad.
    [many] los que se eliminan fueron copiados fuera de tu dispositivo por tu copia de seguridad.
   *[other] los que se eliminan fueron copiados fuera de tu dispositivo por tu copia de seguridad.
}

journal.window.home.originals.confirm.mixed.delete.third = { $quantity ->
    [one] eliminarlo no se puede deshacer.
    [many] eliminarlos no se puede deshacer.
   *[other] eliminarlos no se puede deshacer.
}

journal.window.home.originals.confirm.mixed.summary = esto elimina { $deleted_count } de estos originales y mueve { $trashed_count } a la papelera. no va nada más con ellos.

journal.window.home.originals.confirm.mixed.trash.first = { $quantity ->
    [one] el que va a la papelera espera ahí, luego la papelera lo borra y su espacio vuelve.
    [many] los que van a la papelera esperan ahí, luego la papelera los borra y su espacio vuelve.
   *[other] los que van a la papelera esperan ahí, luego la papelera los borra y su espacio vuelve.
}

journal.window.home.originals.confirm.mixed.trash.until.erase.first = { $quantity ->
    [one] el que se mueve a la papelera se borra después de { $days_quantity ->
        [one] 1 día
        [many] { $retention_days } días
       *[other] { $retention_days } días
    }, y su espacio vuelve entonces.
    [many] los que se mueven a la papelera se borran después de { $days_quantity ->
        [one] 1 día
        [many] { $retention_days } días
       *[other] { $retention_days } días
    }, y su espacio vuelve entonces.
   *[other] los que se mueven a la papelera se borran después de { $days_quantity ->
        [one] 1 día
        [many] { $retention_days } días
       *[other] { $retention_days } días
    }, y su espacio vuelve entonces.
}

journal.window.home.originals.confirm.trash.named.stream.third = { $quantity ->
    [one] espera en la papelera, luego la papelera lo borra y su espacio vuelve.
    [many] esperan en la papelera, luego la papelera los borra y su espacio vuelve.
   *[other] esperan en la papelera, luego la papelera los borra y su espacio vuelve.
}

journal.window.home.originals.confirm.trash.until.erase.default.stream.first = { $quantity ->
    [one] esto mueve 1 original de { $date_and_time } a la papelera.
    [many] esto mueve { $original_count } originales de { $date_and_time } a la papelera.
   *[other] esto mueve { $original_count } originales de { $date_and_time } a la papelera.
}

journal.window.home.originals.confirm.trash.until.erase.named.stream.first = { $quantity ->
    [one] esto mueve 1 original de { $date_and_time } · { $stream_name } a la papelera.
    [many] esto mueve { $original_count } originales de { $date_and_time } · { $stream_name } a la papelera.
   *[other] esto mueve { $original_count } originales de { $date_and_time } · { $stream_name } a la papelera.
}

journal.window.home.originals.confirm.trash.until.erase.named.stream.fourth = { $quantity ->
    [one] puedes recuperarlo hasta entonces.
    [many] puedes recuperarlos hasta entonces.
   *[other] puedes recuperarlos hasta entonces.
}

journal.window.home.originals.confirm.trash.until.erase.named.stream.third = { $quantity ->
    [one] la papelera lo borra después de { $days_quantity ->
        [one] 1 día
        [many] { $retention_days } días
       *[other] { $retention_days } días
    }, y su espacio vuelve entonces.
    [many] la papelera los borra después de { $days_quantity ->
        [one] 1 día
        [many] { $retention_days } días
       *[other] { $retention_days } días
    }, y su espacio vuelve entonces.
   *[other] la papelera los borra después de { $days_quantity ->
        [one] 1 día
        [many] { $retention_days } días
       *[other] { $retention_days } días
    }, y su espacio vuelve entonces.
}

journal.window.home.originals.copy.bulk.clear = limpiar la selección

journal.window.home.originals.copy.bulk.delete = eliminar lo seleccionado

journal.window.home.originals.copy.bulk.keep = conservar lo seleccionado

journal.window.home.originals.copy.bulk.select.all = seleccionar esta página

journal.window.home.originals.copy.card.empty = nada te está esperando.

journal.window.home.originals.copy.card.heading = originales que te esperan

journal.window.home.originals.copy.card.loading = cargando originales…

journal.window.home.originals.copy.card.subhead = audio y video originales marcados para eliminación, más lo que una eliminación dejó sin terminar. nada de lo marcado aquí se elimina hasta que lo digas.

journal.window.home.originals.copy.card.support = contactar a soporte para pedir ayuda

journal.window.home.originals.copy.card.too.large = los detalles de eliminación de este elemento son demasiado grandes para revisarlos aquí.

journal.window.home.originals.copy.card.unavailable = no se pudo cargar la lista.

journal.window.home.originals.copy.confirm.cancel = cancelar

journal.window.home.originals.copy.confirm.go.trash = mover a la papelera

journal.window.home.originals.copy.confirm.recover.body = esto termina cada eliminación que se detuvo a medias. cualquier original que todavía no se había eliminado vuelve a su segmento. el resto sigue eliminado, y los que marcó tu configuración de retención están en la papelera.

journal.window.home.originals.copy.confirm.recover.go = terminarlas

journal.window.home.originals.copy.confirm.recover.heading = ¿terminar las eliminaciones que se detuvieron?

journal.window.home.originals.copy.done.clause.halted = se detuvo ahí.

journal.window.home.originals.copy.done.clause.nothing.deleted = no se eliminó nada.

journal.window.home.originals.copy.done.declined.failed = no se pudo quitar de la lista, así que se queda. no se eliminó nada.

journal.window.home.originals.copy.done.declined.unknown = no se eliminó nada. no está claro si sigue en la lista.

journal.window.home.originals.copy.done.kept.offload = se conservó por ahora. volverá después de tu próxima copia de seguridad.

journal.window.home.originals.copy.done.kept.policy = se conservó por ahora. volverá la próxima vez que esta parte de la lista se reconstruya.

journal.window.home.originals.copy.done.open.trash = abrir la papelera

journal.window.home.originals.copy.done.recover.failed = no se pudieron terminar. lo que queda sigue ahí.

journal.window.home.originals.copy.done.recover.unknown = la finalización se detuvo antes de poder confirmar qué pasó. no está claro qué se completó.

journal.window.home.originals.copy.done.recovered = las eliminaciones que se habían detenido ya terminaron.

journal.window.home.originals.copy.done.recovered.leftover = algunas terminaron. al menos una eliminación que se detuvo sigue en espera.

journal.window.home.originals.copy.done.recovered.none = no quedaba nada por terminar.

journal.window.home.originals.copy.done.unknown = la eliminación se detuvo antes de poder confirmar qué pasó. no está claro qué se eliminó y qué no.

journal.window.home.originals.copy.failed.badge = no terminó

journal.window.home.originals.copy.failed.body = una eliminación que iniciaste se detuvo a medias. terminarla devuelve cualquier original que todavía no se había eliminado, y el resto sigue eliminado.

journal.window.home.originals.copy.failed.finish = terminar la eliminación

journal.window.home.originals.copy.operation.completed = la eliminación terminó.

journal.window.home.originals.copy.operation.held = la eliminación no terminó del todo.

journal.window.home.originals.copy.operation.record.unavailable = el registro de la eliminación no está disponible.

journal.window.home.originals.copy.operation.refused = la eliminación no empezó.

journal.window.home.originals.copy.operation.running = la eliminación está en curso.

journal.window.home.originals.copy.operation.stop = dejar de eliminar

journal.window.home.originals.copy.operation.stop.pending = pidiendo una detención.

journal.window.home.originals.copy.operation.stopping = se pidió detener. en espera de que el trabajo termine.

journal.window.home.originals.copy.operation.submitting = enviando tu solicitud de eliminación.

journal.window.home.originals.copy.operation.unavailable = no se pudo revisar el resultado actual.

journal.window.home.originals.copy.operation.unknown = no está claro qué se eliminó y qué no.

journal.window.home.originals.copy.page.selection = la selección vale para esta página y se quita cuando cambias de página.

journal.window.home.originals.copy.row.delete = eliminar originales

journal.window.home.originals.copy.row.keep = conservar por ahora

journal.window.home.originals.heading.count = { $quantity ->
    [one] originales · 1 elemento
    [many] originales · { $count } elementos
   *[other] originales · { $count } elementos
}

journal.window.home.originals.page.item.range = elementos { $first_item }–{ $last_item } de { $total_items }

journal.window.home.originals.receipt.deleted = { $quantity ->
    [one] se eliminó 1 original.
    [many] se eliminaron { $original_count } originales.
   *[other] se eliminaron { $original_count } originales.
}

journal.window.home.originals.receipt.kept = { $quantity ->
    [one] se conservó 1 por ahora.
    [many] se conservaron { $original_count } por ahora.
   *[other] se conservaron { $original_count } por ahora.
}

journal.window.home.originals.receipt.moved.to.trash = { $quantity ->
    [one] se movió 1 original a la papelera.
    [many] se movieron { $original_count } originales a la papelera.
   *[other] se movieron { $original_count } originales a la papelera.
}

journal.window.home.originals.receipt.none.deleted = { $quantity ->
    [one] no se pudo eliminar. no se eliminó nada.
    [many] no se pudieron eliminar. no se eliminó nada.
   *[other] no se pudieron eliminar. no se eliminó nada.
}

journal.window.home.originals.receipt.not.kept = { $quantity ->
    [one] 1 más no se pudo quitar de la lista, así que se queda.
    [many] { $remaining_count } más no se pudieron quitar de la lista, así que se quedan.
   *[other] { $remaining_count } más no se pudieron quitar de la lista, así que se quedan.
}

journal.window.home.originals.receipt.not.removed = { $quantity ->
    [one] 1 más no se pudo eliminar.
    [many] { $remaining_count } más no se pudieron eliminar.
   *[other] { $remaining_count } más no se pudieron eliminar.
}

journal.window.home.originals.receipt.refused.named = { $original_name }: { $reason }

journal.window.home.originals.receipt.refused.reason = { $reason }

journal.window.home.originals.receipt.space.after.erase = { $quantity ->
    [one] se borra { $erase_day }, y entonces vuelve su espacio. puedes borrarlo antes desde la papelera.
    [many] se borran { $erase_day }, y entonces vuelve su espacio. puedes borrarlos antes desde la papelera.
   *[other] se borran { $erase_day }, y entonces vuelve su espacio. puedes borrarlos antes desde la papelera.
}

journal.window.home.originals.receipt.space.without.erase.day = { $quantity ->
    [one] su espacio vuelve cuando se borra de la papelera.
    [many] su espacio vuelve cuando se borran de la papelera.
   *[other] su espacio vuelve cuando se borran de la papelera.
}

journal.window.home.originals.review.unfinished = revisar originales · las eliminaciones sin terminar necesitan revisión

journal.window.home.originals.row.contents.count = { $quantity ->
    [one] 1 original · { $size }
    [many] { $original_count } originales · { $size }
   *[other] { $original_count } originales · { $size }
}

journal.window.home.originals.row.copied.by.backup = { $quantity ->
    [one] 1 original copiado fuera de tu dispositivo por tu copia de seguridad.
    [many] { $original_count } originales copiados fuera de tu dispositivo por tu copia de seguridad.
   *[other] { $original_count } originales copiados fuera de tu dispositivo por tu copia de seguridad.
}

journal.window.home.originals.row.copied.by.backup.detail = { $quantity ->
    [one] no hay forma de revisar que siga ahí.
    [many] no hay forma de revisar que sigan ahí.
   *[other] no hay forma de revisar que sigan ahí.
}

journal.window.home.originals.row.identity.stream = { $date_and_time } · { $stream_name }

journal.window.home.originals.row.marked.by.retention = { $quantity ->
    [one] 1 original marcado por tu configuración de retención.
    [many] { $original_count } originales marcados por tu configuración de retención.
   *[other] { $original_count } originales marcados por tu configuración de retención.
}

journal.window.home.originals.row.marked.by.retention.detail = esta parte de la lista se reconstruye cada día.

journal.window.home.originals.row.nothing.else.goes = { $quantity ->
    [one] no va nada más con él.
    [many] no va nada más con ellos.
   *[other] no va nada más con ellos.
}

journal.window.home.originals.selection.label = { $checkbox } seleccionar este elemento

journal.window.home.response.briefing.invalid = Respuesta de resumen mal formada

journal.window.home.response.pulse.failed = No se pudo cargar el pulso

journal.window.home.response.pulse.invalid = Respuesta de pulso mal formada

journal.window.home.response.yesterday.failed = no se pudo cargar ayer.

journal.window.home.section.heading.connections = conexiones

journal.window.home.section.heading.getting.started = primeros pasos

journal.window.home.section.heading.next = siguiente

journal.window.home.section.heading.your.week = tu semana

journal.window.home.security.device = a la app de solstone en{ $slot1 }{ $device }{ $slot2 }le falta un arreglo de seguridad. actualízala a{ $slot3 }{ $version }{ $slot4 }o posterior.

journal.window.home.security.journal = a tu diario le falta un arreglo de seguridad. actualízalo a{ $slot1 }{ $version }{ $slot2 }o posterior.

journal.window.home.security.unnamed.device = a la app de solstone en uno de tus dispositivos le falta un arreglo de seguridad. actualízala a { $version } o posterior.

journal.window.home.setup.action.choose = elegir →

journal.window.home.setup.action.finish = terminar →

journal.window.home.setup.agent.choose.connector = en{ $slot1 }{ $agent }, elige

journal.window.home.setup.agent.description = deja que{ $slot1 }{ $agent }{ $slot2 }u otro agente lea tu diario.

journal.window.home.setup.phone.download = consigue solstone en tu teléfono en{ $slot1 }{ $site }, y luego vincúlalo con tu diario.

journal.window.home.setup.speech.package.repair = repara o reinstala la app del diario y luego reinicia tu diario.

journal.window.home.text.backup.status.is.unavailable = el estado de la copia de seguridad no está disponible.

journal.window.home.text.no.time.measured = sin tiempo medido

journal.window.home.text.originals.the.list.couldn.t.be = originales · no se pudo cargar la lista.

journal.window.home.text.queued.work.status.is.unavailable = el estado del trabajo en cola no está disponible.

journal.window.home.text.service.status.is.unavailable = el estado del servicio no está disponible.

journal.window.home.text.the.backup.key.couldn.t.open = no se pudo abrir el destino de la copia de seguridad con la clave. revisa la copia de seguridad en salud.

journal.window.home.time.ago.day = { $quantity ->
    [one] hace { $count } día
    [many] hace { $count } días
   *[other] hace { $count } días
}

journal.window.home.time.ago.hour = { $quantity ->
    [one] hace { $count } hora
    [many] hace { $count } horas
   *[other] hace { $count } horas
}

journal.window.home.time.ago.minute = { $quantity ->
    [one] hace { $count } minuto
    [many] hace { $count } minutos
   *[other] hace { $count } minutos
}

journal.window.home.time.ago.month = { $quantity ->
    [one] hace { $count } mes
    [many] hace { $count } meses
   *[other] hace { $count } meses
}

journal.window.home.time.ago.second = { $quantity ->
    [one] hace { $count } segundo
    [many] hace { $count } segundos
   *[other] hace { $count } segundos
}

journal.window.home.time.ago.week = { $quantity ->
    [one] hace { $count } semana
    [many] hace { $count } semanas
   *[other] hace { $count } semanas
}

journal.window.home.today.activity.count = { $quantity ->
    [one] 1 actividad
    [many] { $activity_count } actividades
   *[other] { $activity_count } actividades
}

journal.window.home.today.activity.preview.count = { $quantity ->
    [one] { $preview_count } de { $activity_count } actividad
    [many] { $preview_count } de { $activity_count } actividades
   *[other] { $preview_count } de { $activity_count } actividades
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
    [one] tiempo en cada faceta · medido para { $measured_count } de 1 actividad
    [many] tiempo en cada faceta · medido para { $measured_count } de { $activity_count } actividades
   *[other] tiempo en cada faceta · medido para { $measured_count } de { $activity_count } actividades
}

journal.window.home.visible.text.add.your.phone = agregar tu teléfono

journal.window.home.visible.text.and.authenticate = y autentícate.

journal.window.home.visible.text.choose.another.way.when.you.re = elige otra forma cuando quieras. tu diario guarda todo hasta entonces.

journal.window.home.visible.text.connect.an.agent = conectar un agente

journal.window.home.visible.text.connections.are.unavailable.right.now = las conexiones no están disponibles ahora mismo.

journal.window.home.visible.text.connections.found.in.your.journal = conexiones encontradas en tu diario

journal.window.home.visible.text.couldn.t.be.written.out = no se pudo redactar.

journal.window.home.visible.text.downloading.the.local.model = descargando el modelo local

journal.window.home.visible.text.downloading.the.speech.model = descargando el modelo de habla

journal.window.home.visible.text.from = de

journal.window.home.visible.text.from.your.devices.and.apps = de tus dispositivos y aplicaciones

journal.window.home.visible.text.hasn.t.been.written.out.yet = todavía no se redactó.

journal.window.home.visible.text.it.appears.here.as.text = aparece aquí como texto.

journal.window.home.visible.text.leave.out.of.this.week = dejar fuera de esta semana

journal.window.home.visible.text.left.out.of.this.week = fuera de esta semana ·

journal.window.home.visible.text.loading.your.week = cargando tu semana…

journal.window.home.visible.text.next = siguiente

journal.window.home.visible.text.no.direct.connections.yet = todavía no hay conexiones directas.

journal.window.home.visible.text.no.named.connections.yet = todavía no hay conexiones con nombre.

journal.window.home.visible.text.no.recent.activity.yet.today = todavía no hay actividad reciente hoy.

journal.window.home.visible.text.no.speech.was.found.in.the = no se encontró habla en el segmento de

journal.window.home.visible.text.nothing.from.yesterday.to.write.up = nada de ayer para redactar.

journal.window.home.visible.text.nothing.from.yesterday.was.written.up = no se redactó nada de ayer.

journal.window.home.visible.text.nothing.with.sound.has.reached.your = todavía no llegó nada con sonido a tu diario. activa el micrófono en la app de solstone para probar esto.

journal.window.home.visible.text.often.around.events.only = a menudo cerca, solo eventos

journal.window.home.visible.text.on.your.calendar.only = solo en tu calendario

journal.window.home.visible.text.on.your.journal.s.computer.run = en la computadora de tu diario, ejecuta:

journal.window.home.visible.text.only.this.week.changes.the.memory = solo cambia esta semana. el recuerdo se queda en tu diario.

journal.window.home.visible.text.opens.where.it.came.from.so = se abre de donde vino, para que puedas revisarlo.

journal.window.home.visible.text.previous = anterior

journal.window.home.visible.text.say.a.sentence.out.loud = di una frase en voz alta

journal.window.home.visible.text.some.source.material.was.unavailable = parte del material de origen no estaba disponible:

journal.window.home.visible.text.speech.from = habla de

journal.window.home.visible.text.the.local.model.didn.t.finish = el modelo local no terminó de descargarse.

journal.window.home.visible.text.the.segment.from = el segmento de

journal.window.home.visible.text.the.speech.model.didn.t.finish = el modelo de habla no terminó de descargarse.

journal.window.home.visible.text.then.type = luego escribe

journal.window.home.visible.text.this.isn.t.right = esto no está bien

journal.window.home.visible.text.try.again = reintentar

journal.window.home.visible.text.waiting.for.the.next.segment.this = en espera del siguiente segmento. esto puede tardar unos minutos.

journal.window.home.visible.text.was.written.out = se redactó.

journal.window.home.visible.text.what.this.summary.covers.wasn.t = lo que cubre este resumen no se guardó con él.

journal.window.home.visible.text.yesterday.is.being.written.up = ayer se está redactando…

journal.window.home.visible.text.your.journal.keeps.everything.until.you = tu diario guarda todo hasta que lo hagas.

journal.window.home.vitals.last.check.timestamp = última revisión: { $checked_at }.

journal.window.home.vitals.last.known = último conocido:{ $slot1 }{ $issue }

journal.window.home.vitals.last.reached.timestamp = llegó por última vez a tu diario { $last_reached_at }

journal.window.home.week.card.empty = no se muestra nada de esta semana.

journal.window.home.week.card.first.choose = tu primera semana llega cuando elijas dónde ocurre el razonamiento.

journal.window.home.week.card.first.due.friday = tu primera semana llega el viernes.

journal.window.home.week.card.first.due.monday = tu primera semana llega el lunes.

journal.window.home.week.card.first.due.saturday = tu primera semana llega el sábado.

journal.window.home.week.card.first.due.sunday = tu primera semana llega el domingo.

journal.window.home.week.card.first.due.thursday = tu primera semana llega el jueves.

journal.window.home.week.card.first.due.tuesday = tu primera semana llega el martes.

journal.window.home.week.card.first.due.wednesday = tu primera semana llega el miércoles.

journal.window.home.week.card.next.choose = tu próxima semana llega cuando elijas dónde ocurre el razonamiento.

journal.window.home.week.card.next.due.friday = tu próxima semana llega el viernes.

journal.window.home.week.card.next.due.monday = tu próxima semana llega el lunes.

journal.window.home.week.card.next.due.saturday = tu próxima semana llega el sábado.

journal.window.home.week.card.next.due.sunday = tu próxima semana llega el domingo.

journal.window.home.week.card.next.due.thursday = tu próxima semana llega el jueves.

journal.window.home.week.card.next.due.tuesday = tu próxima semana llega el martes.

journal.window.home.week.card.next.due.wednesday = tu próxima semana llega el miércoles.

journal.window.home.week.card.unchecked = no se pudo revisar tu semana.

journal.window.home.week.cell.accessible.left.out = { $weekday } { $day }, en tu diario, sin recuerdo en esta página

journal.window.home.week.cell.accessible.memory = { $weekday } { $day }, un recuerdo de este día

journal.window.home.week.cell.accessible.not.ready = { $weekday } { $day }, no estuvo listo a tiempo

journal.window.home.week.cell.accessible.nothing.shared = { $weekday } { $day }, nada en tu diario

journal.window.home.week.cell.accessible.unknown = { $weekday } { $day },{ $legacy_space }

journal.window.home.week.cell.accessible.unreadable = { $weekday } { $day }, no se pudo leer

journal.window.home.week.cell.left.out = en tu diario, ningún recuerdo en esta página

journal.window.home.week.cell.memory = un recuerdo de este día

journal.window.home.week.cell.not.ready = no estuvo listo a tiempo

journal.window.home.week.cell.nothing.shared = nada en tu diario

journal.window.home.week.cell.unreadable = no se pudo leer

journal.window.home.week.day.long.friday = viernes

journal.window.home.week.day.long.monday = lunes

journal.window.home.week.day.long.saturday = sábado

journal.window.home.week.day.long.sunday = domingo

journal.window.home.week.day.long.thursday = jueves

journal.window.home.week.day.long.tuesday = martes

journal.window.home.week.day.long.wednesday = miércoles

journal.window.home.week.day.short.fri = vie

journal.window.home.week.day.short.mon = lun

journal.window.home.week.day.short.sat = sáb

journal.window.home.week.day.short.sun = dom

journal.window.home.week.day.short.thu = jue

journal.window.home.week.day.short.tue = mar

journal.window.home.week.day.short.wed = mié

journal.window.home.week.day.short.with.number = { $weekday}{ $slot1}{ $day}

journal.window.home.week.days.conjunction = { $leading }{ $slot1 }y{ $slot2 }{ $last }

journal.window.home.week.days.from = de{ $slot1 }{ $days }.

journal.window.home.week.days.unreadable = algo de{ $slot1 }{ $days }{ $slot2 }no se pudo leer.

journal.window.home.week.days.unready = { $quantity ->
    [one] { $days } no estuvo listo a tiempo.
    [many] { $days } no estuvieron listos a tiempo.
   *[other] { $days } no estuvieron listos a tiempo.
}

journal.window.home.week.end = esa es la semana.

journal.window.home.week.intro.empty = no hay nada de esta semana en esta página.

journal.window.home.week.intro.memories = { $quantity ->
    [one] tu semana, un recuerdo de un día.
    [many] tu semana, un recuerdo de cada uno de { $count } de días.
   *[other] tu semana, un recuerdo de cada uno de { $count } días.
}

journal.window.home.week.leave.out.apply.failed = no se pudo dejar esto afuera. no cambió nada.

journal.window.home.week.leave.out.undo.failed = no se pudo traer esto de vuelta. no cambió nada.

journal.window.home.week.left.out.unreadable = no se pudieron revisar tus recuerdos dejados afuera, así que se muestra todo.

journal.window.home.week.load.failed = no se pudo cargar esta semana. recarga para volver a intentarlo.

journal.window.home.week.menu.accessible.name = más para{ $slot1 }{ $day }

journal.window.home.week.month.april = abril

journal.window.home.week.month.august = agosto

journal.window.home.week.month.december = diciembre

journal.window.home.week.month.february = febrero

journal.window.home.week.month.january = enero

journal.window.home.week.month.july = julio

journal.window.home.week.month.june = junio

journal.window.home.week.month.march = marzo

journal.window.home.week.month.may = mayo

journal.window.home.week.month.november = noviembre

journal.window.home.week.month.october = octubre

journal.window.home.week.month.september = septiembre

journal.window.home.week.navigation.next = { $week } →

journal.window.home.week.navigation.previous = ← { $week }

journal.window.home.week.peek.briefing = del resumen diario de{ $slot1 }{ $day }

journal.window.home.week.peek.moment = abrir ese momento →

journal.window.home.week.peek.thinking = abrirlo en razonamiento →

journal.window.home.week.said.heading = dicho por ti

journal.window.home.week.said.leave.out.scope = solo cambia esta semana. lo que dijiste permanece en tu diario.

journal.window.home.week.said.open = abrir donde lo dijiste →

journal.window.home.week.said.unreadable = no se pudo leer la lista "dicho por ti".

journal.window.home.week.source.link = fuente

journal.window.home.week.title.current = semana de{ $slot1 }{ $month }{ $slot2 }{ $day }

journal.window.home.week.title.with.year = semana de{ $slot1 }{ $month }{ $slot2 }{ $day },{ $slot3 }{ $year }

journal.window.home.week.unreadable.current = no se pudo leer la semana de{ $slot1 }{ $month }{ $slot2 }{ $day }{ $slot3 }

journal.window.home.week.unreadable.with.year = no se pudo leer la semana de{ $slot1 }{ $month }{ $slot2 }{ $day },{ $slot3 }{ $year }

journal.window.home.welcome.first.week.framing = la mayor parte de lo que tu diario guarda se vuelve útil tras cerca de una semana, cuando tu diario tiene suficientes de tus días en él para mostrar patrones. por ahora, esto es lo que ya está pasando:

journal.window.home.window.explanation.analysis.will.be.available.after.the = el análisis estará disponible después del próximo ciclo de procesamiento.

journal.window.home.window.explanation.backup.status.is.unavailable.right.now = el estado de la copia de seguridad no está disponible ahora mismo.

journal.window.home.window.explanation.choose.where.the.thinking.happens = elige dónde ocurre el razonamiento

journal.window.home.window.explanation.couldn.t.bring.this.back.nothing = no se pudo traer esto de vuelta. no cambió nada.

journal.window.home.window.explanation.couldn.t.leave.this.out.nothing = no se pudo dejar esto afuera. no cambió nada.

journal.window.home.window.explanation.couldn.t.refresh.narrative.showing.last = no se pudo actualizar la narrativa. mostrando el último estado conocido.

journal.window.home.window.explanation.couldn.t.refresh.service.status.showing = no se pudo actualizar el estado del servicio. mostrando el último estado conocido.

journal.window.home.window.explanation.finish.setting.up.confidential.processing = termina de configurar el procesamiento confidencial

journal.window.home.window.explanation.finish.setting.up.local.thinking = termina de configurar el razonamiento local

journal.window.home.window.explanation.finish.setting.up.your.own.model = termina de configurar tu propio modelo

journal.window.home.window.explanation.no.narrative.yet.one.will.appear = todavía no hay narrativa. aparecerá una después de que entre algo de audio en tu diario.

journal.window.home.window.explanation.queued.work.status.is.unavailable.right = el estado del trabajo en cola no está disponible ahora mismo.

journal.window.home.window.explanation.reload.to.try.again = recarga para volver a intentarlo.

journal.window.home.window.explanation.service.status.is.unavailable.right.now = el estado del servicio no está disponible ahora mismo.

journal.window.home.window.explanation.this.is.where.your.day.comes = aquí es donde tu día se junta: qué pasó, redactado, y las personas y las cosas que involucró. las secciones aparecen aquí a medida que lo que compartes con la app de solstone entra en tu diario.

journal.window.home.window.explanation.time.in.each.facet = tiempo en cada faceta

journal.window.home.window.explanation.yesterday.shows.here.once.confidential.processing = ayer aparece aquí cuando el procesamiento confidencial esté configurado.

journal.window.home.window.explanation.yesterday.shows.here.once.local.thinking = ayer aparece aquí cuando el razonamiento local esté configurado.

journal.window.home.window.explanation.yesterday.shows.here.once.you.choose = ayer aparece aquí cuando elijas dónde ocurre el razonamiento.

journal.window.home.window.explanation.yesterday.shows.here.once.your.own = ayer aparece aquí cuando tu propio modelo esté configurado.

journal.window.home.window.explanation.yesterday.shows.here.the.morning.after = ayer aparece aquí la mañana después de que algo entre en tu diario.

journal.window.home.window.explanation.your.first.day.shows.here.by = tu primer día aparece aquí para las 10 de la mañana, después de la primera noche de tu diario.

journal.window.home.window.explanation.your.journal.s.computer.can.t = la computadora de tu diario no puede ejecutar el razonamiento local

journal.window.loading.activities = cargando actividades…

journal.window.loading.agents = cargando agentes…

journal.window.loading.backup = cargando copia de seguridad…

journal.window.loading.day = cargando el día…

journal.window.loading.entities = cargando entidades…

journal.window.loading.facets = cargando facetas…

journal.window.loading.health = cargando salud…

journal.window.loading.home = cargando tu día…

journal.window.loading.import = cargando importaciones…

journal.window.loading.map = cargando mapa…

journal.window.loading.more = cargando más…

journal.window.loading.network = cargando dispositivos…

journal.window.loading.profile = cargando perfil…

journal.window.loading.records = cargando registros…

journal.window.loading.search = cargando búsqueda…

journal.window.loading.settings = cargando la configuración…

journal.window.loading.sources = cargando fuentes…

journal.window.loading.speakers = cargando hablantes…

journal.window.loading.stats = cargando estadísticas…

journal.window.loading.support = cargando soporte…

journal.window.loading.thinking = cargando la configuración de razonamiento…

journal.window.loading.transcripts = cargando transcripciones…

journal.window.loading.year = cargando año…

journal.window.more.agents.count = { $quantity ->
    [one] { $count } conectado
    [many] { $count } de conectados
   *[other] { $count } conectados
}

journal.window.more.coverage.since = desde{ $slot1 }{ $date }

journal.window.more.entities.count = { $quantity ->
    [one] { $count } guardada
    [many] { $count } de guardadas
   *[other] { $count } guardadas
}

journal.window.more.field.label.filter.views = filtrar vistas

journal.window.more.field.placeholder.type.to.filter.views = escribe para filtrar vistas

journal.window.more.group.runs = cómo se ejecuta

journal.window.more.group.shape = lo que moldeas

journal.window.more.imports.count = { $quantity ->
    [one] 1 importación
    [many] { $count } de importaciones
   *[other] { $count } importaciones
}

journal.window.more.page.heading.more = más

journal.window.more.pin.action.pin = { $view ->
        [agents] anclar agentes en la barra lateral
        [activities] anclar actividades en la barra lateral
        [backup] anclar copia de seguridad en la barra lateral
        [body] anclar cuerpo en la barra lateral
        [curation] anclar sugerencias en la barra lateral
        [entities] anclar entidades en la barra lateral
        [health] anclar salud en la barra lateral
        [home] anclar inicio en la barra lateral
        [import] anclar importación en la barra lateral
        [map] anclar mapa en la barra lateral
        [more] anclar más en la barra lateral
        [network] anclar red en la barra lateral
        [search] anclar búsqueda en la barra lateral
        [settings] anclar configuración en la barra lateral
        [sources] anclar fuentes en la barra lateral
        [speakers] anclar hablantes en la barra lateral
        [stats] anclar estadísticas en la barra lateral
        [support] anclar soporte en la barra lateral
        [thinking] anclar razonamiento en la barra lateral
        [transcripts] anclar transcripciones en la barra lateral
        [year] anclar año en la barra lateral
       *[other] anclar { $view_name } en la barra lateral
    }

journal.window.more.pin.action.unpin = { $view ->
        [agents] desanclar agentes de la barra lateral
        [activities] desanclar actividades de la barra lateral
        [backup] desanclar copia de seguridad de la barra lateral
        [body] desanclar cuerpo de la barra lateral
        [curation] desanclar sugerencias de la barra lateral
        [entities] desanclar entidades de la barra lateral
        [health] desanclar salud de la barra lateral
        [home] desanclar inicio de la barra lateral
        [import] desanclar importación de la barra lateral
        [map] desanclar mapa de la barra lateral
        [more] desanclar más de la barra lateral
        [network] desanclar red de la barra lateral
        [search] desanclar búsqueda de la barra lateral
        [settings] desanclar configuración de la barra lateral
        [sources] desanclar fuentes de la barra lateral
        [speakers] desanclar hablantes de la barra lateral
        [stats] desanclar estadísticas de la barra lateral
        [support] desanclar soporte de la barra lateral
        [thinking] desanclar razonamiento de la barra lateral
        [transcripts] desanclar transcripciones de la barra lateral
        [year] desanclar año de la barra lateral
       *[other] desanclar { $view_name } de la barra lateral
    }

journal.window.more.review.count = { $quantity ->
    [one] { $count } para revisar
    [many] { $count } para revisar
   *[other] { $count } para revisar
}

journal.window.more.suggestions.pending = { $count }{ $slot1 }para revisar

journal.window.more.text.no.devices.yet = todavía no hay dispositivos

journal.window.more.text.none.yet = todavía no hay

journal.window.more.text.not.set.up.yet = todavía sin configurar

journal.window.more.text.nothing.to.review = nada para revisar

journal.window.more.text.nothing.yet = todavía no hay nada

journal.window.more.tooltip.always.in.the.side.rail = siempre en la barra lateral

journal.window.more.visible.text.couldn.t.save.that.try.again = no se pudo guardar eso. vuelve a intentarlo.

journal.window.more.visible.text.every.view.your.journal.has.what = cada vista que tiene tu diario: qué muestra cada una y de dónde viene su contenido.

journal.window.more.visible.text.no.views.match.that = ninguna vista coincide con eso.

journal.window.more.window.explanation.fix.and.tidy.what.it.keeps = arreglar y ordenar lo que guarda

journal.window.more.window.explanation.set.it.up.and.keep.it = configurarlo y mantenerlo en marcha

journal.window.more.window.explanation.what.your.journal.keeps = lo que tu diario guarda

journal.window.more.window.explanation.your.days.as.your.journal.holds = tus días, como los guarda tu diario

journal.window.search.agent.action = acción

journal.window.search.agent.activity = actividad

journal.window.search.agent.audio = audio

journal.window.search.agent.audio.transcript = transcripción de audio

journal.window.search.agent.body = cuerpo

journal.window.search.agent.brief = resumen

journal.window.search.agent.browser = navegador

journal.window.search.agent.calendar = calendario

journal.window.search.agent.conversation.transcript = transcripción de conversación

journal.window.search.agent.describe = describir

journal.window.search.agent.desktop.activity = actividad de escritorio

journal.window.search.agent.document.transcript = transcripción de documento

journal.window.search.agent.documents = documentos

journal.window.search.agent.entity.attached = entidad:adjunta

journal.window.search.agent.entity.detected = entidad:detectada

journal.window.search.agent.event = evento

journal.window.search.agent.event.transcript = transcripción de evento

journal.window.search.agent.flow = flujo

journal.window.search.agent.granola = granola

journal.window.search.agent.image.transcript = transcripción de imagen

journal.window.search.agent.import = importación

journal.window.search.agent.imported = importado

journal.window.search.agent.location = ubicación

journal.window.search.agent.memory = recuerdo

journal.window.search.agent.morning.briefing = resumen de la mañana

journal.window.search.agent.note.transcript = transcripción de nota

journal.window.search.agent.objects = objetos

journal.window.search.agent.observation = observación

journal.window.search.agent.possible.names = nombres posibles

journal.window.search.agent.protected.video = video protegido

journal.window.search.agent.pulse = pulso

journal.window.search.agent.reflection = reflexión

journal.window.search.agent.screen = pantalla

journal.window.search.agent.screen.text = texto de pantalla

journal.window.search.agent.screenshot.transcript = transcripción de captura de pantalla

journal.window.search.agent.sense = sentido

journal.window.search.agent.session.review = revisión de la sesión

journal.window.search.agent.story = historia

journal.window.search.agent.topics = temas

journal.window.search.count.results.across.days = { $match_quantity ->
    [one] { $day_quantity ->
        [one] { $match_count } coincidencia a lo largo de { $day_count } día.
        [many] { $match_count } coincidencia a lo largo de { $day_count } días.
       *[other] { $match_count } coincidencia a lo largo de { $day_count } días.
    }
    [many] { $day_quantity ->
        [one] { $match_count } coincidencias a lo largo de { $day_count } día.
        [many] { $match_count } coincidencias a lo largo de { $day_count } días.
       *[other] { $match_count } coincidencias a lo largo de { $day_count } días.
    }
   *[other] { $day_quantity ->
        [one] { $match_count } coincidencias a lo largo de { $day_count } día.
        [many] { $match_count } coincidencias a lo largo de { $day_count } días.
       *[other] { $match_count } coincidencias a lo largo de { $day_count } días.
    }
}

journal.window.search.count.results.shown.for.day = { $match_quantity ->
    [one] { $shown_quantity ->
        [one] se muestran las primeras { $shown_count } de { $match_count } coincidencia de este día.
        [many] se muestran las primeras { $shown_count } de { $match_count } coincidencia de este día.
       *[other] se muestran las primeras { $shown_count } de { $match_count } coincidencia de este día.
    }
    [many] { $shown_quantity ->
        [one] se muestran las primeras { $shown_count } de { $match_count } coincidencias de este día.
        [many] se muestran las primeras { $shown_count } de { $match_count } coincidencias de este día.
       *[other] se muestran las primeras { $shown_count } de { $match_count } coincidencias de este día.
    }
   *[other] { $shown_quantity ->
        [one] se muestran las primeras { $shown_count } de { $match_count } coincidencias de este día.
        [many] se muestran las primeras { $shown_count } de { $match_count } coincidencias de este día.
       *[other] se muestran las primeras { $shown_count } de { $match_count } coincidencias de este día.
    }
}

journal.window.search.date.from = desde

journal.window.search.date.invalid = elige una fecha final igual o posterior a la inicial.

journal.window.search.date.range = intervalo de fechas

journal.window.search.date.through = hasta

journal.window.search.entry.as.indexed = según la última indexación

journal.window.search.entry.close = cerrar la entrada

journal.window.search.entry.loading = cargando la entrada…

journal.window.search.entry.open = leer la entrada coincidente

journal.window.search.entry.reindexed = este archivo se indexó de nuevo después de tu búsqueda, así que esto puede no coincidir con el resultado

journal.window.search.entry.stored.record = el registro guardado

journal.window.search.heading = buscar en tu diario

journal.window.search.hit.names = nombres

journal.window.search.intro = encuentra palabras y frases en tu diario, y luego lee la entrada coincidente.

journal.window.search.link.agents = abrir en agentes

journal.window.search.link.day = abrir ese día →

journal.window.search.link.entity = abrir entidad

journal.window.search.link.map = abrir en el mapa

journal.window.search.link.transcript = abrir transcripción

journal.window.search.link.week = abrir esa semana →

journal.window.search.message.assistant = Asistente

journal.window.search.message.everyone = todos

journal.window.search.message.meta = { $sender } a { $recipients }

journal.window.search.message.unknown = Desconocido

journal.window.search.message.you = tú

journal.window.search.paging.accessible.name = páginas de resultados de búsqueda

journal.window.search.paging.day = más de este día

journal.window.search.paging.first = volver a los primeros resultados

journal.window.search.paging.newer = días coincidentes más recientes

journal.window.search.paging.older = días coincidentes más antiguos

journal.window.search.query.label = palabras o frase

journal.window.search.query.placeholder = ¿qué te gustaría encontrar?

journal.window.search.query.submit = buscar

journal.window.search.response.agent.name = el agente debe ser solo el nombre del talento

journal.window.search.response.date.format = { $field }{ $slot1 }debe ser { $slot2 }

journal.window.search.response.date.real = { $field }{ $slot1 }debe ser un día real

journal.window.search.response.date.reverse = { $slot1 } debe ser <= { $slot2 }

journal.window.search.response.day.format = { $slot1 } debe ser { $slot2 }

journal.window.search.response.entry.absent = esta entrada ya no está indexada. busca de nuevo para actualizar los resultados.

journal.window.search.response.entry.invalid = referencia de entrada no válida

journal.window.search.response.entry.large = esta entrada es demasiado larga para mostrarla aquí.

journal.window.search.response.file.not.found = ese archivo no está disponible.

journal.window.search.response.file.read.failed = no se pudo leer ese archivo.

journal.window.search.response.filter.agent = no hay un agente llamado{ $slot1 }{ $name }. agentes:{ $slot2 }{ $available }

journal.window.search.response.filter.facet = no hay una faceta llamada{ $slot1 }{ $name }. facetas:{ $slot2 }{ $available }

journal.window.search.response.index.read.detail = no se pudo leer el índice de búsqueda. vuelve a intentarlo.

journal.window.search.response.invalid.day = no se pudo usar ese día.

journal.window.search.response.invalid.path = no se pudo usar esa ruta.

journal.window.search.response.invalid.value = no se pudo usar uno de esos valores.

journal.window.search.response.read.agent.absent = no se encontró una salida de ese tipo para ese día

journal.window.search.response.read.bound = { $slot1 } debe ser { $slot2 } para lecturas { $slot3 }

journal.window.search.response.read.entity = los resultados de entidad no tienen un archivo para leer

journal.window.search.response.read.entry.fields = una entrada indexada requiere { $slot1 }, { $slot2 } y { $slot3 } del mismo resultado de búsqueda

journal.window.search.response.read.failed = no se pudo leer el archivo del diario

journal.window.search.response.read.file.absent = no se encontró el archivo del diario

journal.window.search.response.read.mixed.entry = una entrada indexada no se puede combinar con { $slot1 }, { $slot2 } o { $slot3 }

journal.window.search.response.read.mixed.file = { $slot1 } no se puede combinar con { $slot2 }, { $slot3 } o { $slot4 }

journal.window.search.response.read.required = { $slot1 } o { $slot2 } es obligatorio

journal.window.search.response.read.suffix = quita el sufijo { $slot1 } del resultado de búsqueda antes de leer una ruta

journal.window.search.response.search.failed = no se pudo buscar en tu diario ahora mismo.

journal.window.search.response.segment.invalid = clave de segmento no válida

journal.window.search.status.details = detalles

journal.window.search.status.entry.failed = no se pudo abrir esta entrada.{ $slot1 }{ $detail }

journal.window.search.status.entry.failure.details = no se pudo abrir esta entrada. { $detail } { $diagnostic }

journal.window.search.status.entry.failure.retry = no se pudo abrir esta entrada. vuelve a intentarlo.

journal.window.search.status.entry.failure.retry.diagnostic = no se pudo abrir esta entrada. vuelve a intentarlo. { $diagnostic }

journal.window.search.status.failure.index.absent = no se pudo buscar en tu diario: tu diario todavía no tiene un índice de búsqueda.

journal.window.search.status.failure.index.empty = no se pudo buscar en tu diario: todavía no hay nada indexado.

journal.window.search.status.failure.index.locked = no se pudo buscar en tu diario: el índice de búsqueda está ocupado ahora mismo.

journal.window.search.status.failure.index.unreadable = no se pudo buscar en tu diario: no se pudo leer el índice de búsqueda.

journal.window.search.status.failure.retry = no se pudo buscar en tu diario. vuelve a intentarlo.

journal.window.search.status.index.rebuilding = la búsqueda se está reconstruyendo y los días más antiguos pueden faltar por ahora.

journal.window.search.status.initial = busca en todo tu diario. entrecomilla una "frase exacta", o limita el rango de días de arriba para resultados más rápidos.

journal.window.search.status.loading = buscando en tu diario…

journal.window.search.status.narrow = agrega palabras para afinar estas coincidencias.

journal.window.search.status.none = no hay coincidencias. prueba con menos palabras o un rango de fechas más amplio.

journal.window.search.status.progress = todavía buscando…{ $slot1 }{ $seconds }{ $slot2 }s.

journal.window.search.status.progress.all.days = todavía buscando…{ $slot1 }{ $seconds }{ $slot2 }s. un solo día suele ser mucho más rápido.

journal.window.search.status.relaxed = nada coincidió con tu búsqueda exacta, así que estas son coincidencias más amplias.

journal.window.search.status.retry = reintentar

journal.window.search.status.search.failed.other = no se pudo buscar en tu diario.{ $slot1 }{ $detail }

journal.window.shared.api.authentication.required = Autenticación necesaria

journal.window.shared.api.malformed.response = Respuesta del servidor mal formada

journal.window.shared.api.request.failed = Falló la solicitud

journal.window.shared.api.request.failed.http = Falló la solicitud (HTTP{ $slot1 }{ $http_status })

journal.window.shared.api.window.explanation.couldn.t.finish.that.request = no se pudo terminar esa solicitud.

journal.window.shared.app.disclosure.label.show.details = mostrar detalles

journal.window.shared.app.label.try.now = Intentar ahora

journal.window.shared.app.notice.background.task = { $app_name }{ $slot1 }tarea en segundo plano

journal.window.shared.app.notification.segments = { $count }{ $slot1 }segmentos

journal.window.shared.app.notification.title = Notificación

journal.window.shared.app.request.failed = Falló la solicitud

journal.window.shared.app.visible.text.click.to.copy = (clic para copiar)

journal.window.shared.app.visible.text.copied = (copiado)

journal.window.shared.app.visible.text.reason.code = código del motivo:

journal.window.shared.app.visible.text.reference = referencia:

journal.window.shared.app.visible.text.server.reason = Motivo del servidor:

journal.window.shared.app.visible.text.still.working = todavía trabajando…

journal.window.shared.app.visible.text.time = hora:

journal.window.shared.app.window.explanation.also.remove.it.from.your.backups = quitarlo también de tus copias de seguridad

journal.window.shared.app.window.explanation.also.remove.them.from.your.backups = quitarlos también de tus copias de seguridad

journal.window.shared.app.window.explanation.backup.is.paused.so.this.happens = la copia de seguridad está en pausa, así que esto ocurre una vez que la reanudes.

journal.window.shared.app.window.explanation.couldn.t.load.this.section = no se pudo cargar esta sección

journal.window.shared.app.window.explanation.couldn.t.reach.your.journal.some = no se pudo conectar con tu diario. algunas actualizaciones en segundo plano están en pausa.

journal.window.shared.app.window.explanation.reload.to.try.again = recarga para volver a intentarlo.

journal.window.shared.app.window.explanation.removing.it.from.your.backups.next = se quitará de tus copias de seguridad a continuación.

journal.window.shared.app.window.explanation.removing.it.from.your.backups.once = se quitará de tus copias de seguridad cuando reanudes la copia de seguridad.

journal.window.shared.app.window.explanation.removing.them.from.your.backups.next = se quitarán de tus copias de seguridad a continuación.

journal.window.shared.app.window.explanation.removing.them.from.your.backups.once = se quitarán de tus copias de seguridad cuando reanudes la copia de seguridad.

journal.window.shared.app.window.explanation.then.restoring.your.journal.from.a = entonces, restaurar tu diario desde una copia de seguridad no puede traerlo de vuelta. tus copias de seguridad se quedan, con todo lo demás en ellas.

journal.window.shared.app.window.explanation.then.restoring.your.journal.from.a.variant2 = entonces, restaurar tu diario desde una copia de seguridad no puede traerlos de vuelta. tus copias de seguridad se quedan, con todo lo demás en ellas.

journal.window.shared.app.window.explanation.your.journal.couldn.t.check.your = tu diario no pudo revisar tus copias de seguridad justo ahora, así que esto las deja como están.

journal.window.shared.console.link.message.count = { $quantity ->
        [one] mensaje del sistema ({ $count })
        [many] mensajes del sistema ({ $count })
       *[other] mensajes del sistema ({ $count })
    }

journal.window.shared.console.link.unread.count = { $quantity ->
        [one] mensajes del sistema, { $count } sin leer
        [many] mensajes del sistema, { $count } sin leer
       *[other] mensajes del sistema, { $count } sin leer
    }

journal.window.shared.console.snapshot.message.count = { $quantity ->
        [one] instantánea de { $count } mensaje del sistema
        [many] instantánea de { $count } mensajes del sistema
       *[other] instantánea de { $count } mensajes del sistema
    }

journal.window.shared.date.format.duration.hour.and.minute.count = { $quantity ->
    [one] { $hours } hr { $minutes_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
    [many] { $hours } hr { $minutes_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
   *[other] { $hours } hr { $minutes_quantity ->
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
        [one] { $seconds } sec
        [many] { $seconds } sec
       *[other] { $seconds } sec
    }
    [many] { $minutes } min { $seconds_quantity ->
        [one] { $seconds } sec
        [many] { $seconds } sec
       *[other] { $seconds } sec
    }
   *[other] { $minutes } min { $seconds_quantity ->
        [one] { $seconds } sec
        [many] { $seconds } sec
       *[other] { $seconds } sec
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

journal.window.shared.date.format.duration.unavailable = duración no disponible

journal.window.shared.date.format.import.audio = audio

journal.window.shared.date.format.import.document = documentos

journal.window.shared.date.format.import.ics = calendario

journal.window.shared.date.format.import.image = imágenes

journal.window.shared.date.format.import.journal.archive = otro diario

journal.window.shared.date.format.import.obsidian = notas

journal.window.shared.date.format.imported.source.label = { $source_name }{ $slot1 }· importado

journal.window.shared.date.format.lane.byo = tu propio modelo

journal.window.shared.date.format.lane.default = procesamiento

journal.window.shared.date.format.lane.local = razonamiento local

journal.window.shared.date.format.lane.spp = procesamiento confidencial

journal.window.shared.date.format.months.since.apr = abr

journal.window.shared.date.format.months.since.aug = ago

journal.window.shared.date.format.months.since.dec = dic

journal.window.shared.date.format.months.since.feb = feb

journal.window.shared.date.format.months.since.jan = ene

journal.window.shared.date.format.months.since.jul = jul

journal.window.shared.date.format.months.since.jun = jun

journal.window.shared.date.format.months.since.mar = mar

journal.window.shared.date.format.months.since.may = may

journal.window.shared.date.format.months.since.nov = nov

journal.window.shared.date.format.months.since.oct = oct

journal.window.shared.date.format.months.since.sep = sep

journal.window.shared.date.format.source.browser = tu navegador

journal.window.shared.date.format.source.phone = tu teléfono

journal.window.shared.date.format.source.terminal = tu terminal

journal.window.shared.date.format.source.unknown = fuente desconocida

journal.window.shared.date.format.source.watch = tu reloj

journal.window.shared.date.format.time.unavailable = hora no disponible

journal.window.shared.date.nav.accessible.name.change.date.level = cambiar el nivel de la fecha

journal.window.shared.date.nav.accessible.name.go.to.today = ir a hoy

journal.window.shared.date.nav.accessible.name.next = siguiente

journal.window.shared.date.nav.accessible.name.next.day = día siguiente

journal.window.shared.date.nav.accessible.name.previous = anterior

journal.window.shared.date.nav.accessible.name.previous.day = día anterior

journal.window.shared.date.nav.cell.date.and.count = { $day_label }, { $quantity_label }

journal.window.shared.date.nav.days.counting = todavía contando los días…

journal.window.shared.date.nav.days.loading = cargando días…

journal.window.shared.date.nav.index.failed.day.count = { $quantity ->
        [one] no se pudo contar 1 día. se sigue intentando.
        [many] no se pudo contar { $count } días. se sigue intentando.
       *[other] no se pudo contar { $count } días. se sigue intentando.
    }

journal.window.shared.date.nav.months.label.april = abril

journal.window.shared.date.nav.months.label.august = agosto

journal.window.shared.date.nav.months.label.december = diciembre

journal.window.shared.date.nav.months.label.february = febrero

journal.window.shared.date.nav.months.label.january = enero

journal.window.shared.date.nav.months.label.july = julio

journal.window.shared.date.nav.months.label.june = junio

journal.window.shared.date.nav.months.label.march = marzo

journal.window.shared.date.nav.months.label.may = mayo

journal.window.shared.date.nav.months.label.november = noviembre

journal.window.shared.date.nav.months.label.october = octubre

journal.window.shared.date.nav.months.label.september = septiembre

journal.window.shared.date.nav.months.loading = cargando meses…

journal.window.shared.date.nav.visible.text.pick.a.day = elige un día

journal.window.shared.date.nav.visible.text.today = hoy

journal.window.shared.date.nav.visible.text.try.again = reintentar

journal.window.shared.date.nav.week.current = Esta semana

journal.window.shared.date.nav.week.headline.month.day = semana de{ $slot1 }{ $month }{ $slot2 }{ $day }

journal.window.shared.date.nav.week.option.month.day = semana de{ $slot1 }{ $month }{ $slot2 }{ $day }

journal.window.shared.date.nav.week.previous = La semana pasada

journal.window.shared.date.nav.window.explanation.couldn.t.load.which.days.have = no se pudo cargar qué días tienen material.

journal.window.shared.date.nav.window.explanation.couldn.t.load.which.months.have = no se pudo cargar qué meses tienen material.

journal.window.shared.day.grid.calendar.year = { $year }{ $slot1 }calendario

journal.window.shared.day.grid.cell.date.and.count = { $day_label }: { $quantity_label }

journal.window.shared.day.grid.cell.date.and.pending.count = { $day_label }: { $quantity_label }, consolidación pendiente

journal.window.shared.day.grid.range.anchor = termina el rango desde{ $slot1 }{ $start_day }

journal.window.shared.day.grid.range.cleared = se limpió el rango

journal.window.shared.day.grid.range.start = el rango empieza{ $slot1 }{ $start_day }. elige el último día.

journal.window.shared.day.grid.visible.text.less = menos

journal.window.shared.day.grid.visible.text.more = más

journal.window.shared.day.grid.visible.text.none = ninguno

journal.window.shared.day.grid.visible.text.open = abrir →

journal.window.shared.day.grid.visible.text.pending.rollup = consolidación pendiente

journal.window.shared.day.grid.visible.text.yes = sí

journal.window.shared.diagnostic.console.reason.code = código del motivo

journal.window.shared.diagnostic.console.server.reason = Motivo del servidor

journal.window.shared.diagnostic.console.visible.text.http = · HTTP

journal.window.shared.error.handler.window.explanation.couldn.t.load.part.of.this = no se pudo cargar parte de esta página.

journal.window.shared.error.handler.window.explanation.navigation.aborted.a.read = la navegación interrumpió una lectura:

journal.window.shared.error.handler.window.explanation.something.on.this.page.didn.t = algo en esta página no funcionó.

journal.window.shared.gate.drawer.audio.recordings.count = { $quantity ->
        [one] grabación con audio: { $count }
        [many] grabaciones con audio: { $count }
       *[other] grabaciones con audio: { $count }
    }

journal.window.shared.gate.drawer.manual.tags.count = { $quantity ->
        [one] frase que enseñaste: { $count }
        [many] frases que enseñaste: { $count }
       *[other] frases que enseñaste: { $count }
    }

journal.window.shared.gate.drawer.median.length = duración mediana

journal.window.shared.gate.drawer.open.label = ¿por qué todavía no?

journal.window.shared.gate.drawer.statement.median.required.seconds = duración mediana de la frase { $observed }s, necesita { $minimum }s

journal.window.shared.gate.drawer.statement.median.seconds = duración mediana de la frase { $observed }s

journal.window.shared.gate.drawer.statement.median.unavailable = duración mediana de la frase n/d

journal.window.shared.gate.drawer.statement.median.unavailable.required.seconds = duración mediana de la frase n/d, necesita { $minimum }s

journal.window.shared.gate.drawer.statements.observed.and.required.count = { $quantity ->
    [one] { $observed } frase más larga hasta ahora, necesita { $minimum_quantity ->
        [one] { $minimum }
        [many] { $minimum }
       *[other] { $minimum }
    }
    [many] { $observed } frases más largas hasta ahora, necesitan { $minimum_quantity ->
        [one] { $minimum }
        [many] { $minimum }
       *[other] { $minimum }
    }
   *[other] { $observed } frases más largas hasta ahora, necesitan { $minimum_quantity ->
        [one] { $minimum }
        [many] { $minimum }
       *[other] { $minimum }
    }
}

journal.window.shared.gate.drawer.statements.observed.count = { $quantity ->
        [one] { $observed } frase más larga hasta ahora
        [many] { $observed } frases más largas hasta ahora
       *[other] { $observed } frases más largas hasta ahora
    }

journal.window.shared.gate.drawer.threshold = necesita { $threshold }

journal.window.shared.gate.drawer.window.explanation.pick.longer.statements.when.you.teach = elige frases más largas cuando enseñes tu voz.

journal.window.shared.gate.drawer.window.explanation.pick.statements.where.only.you.are = elige frases en las que solo tú hables.

journal.window.shared.gate.drawer.window.explanation.teach.a.few.more.of.your = enseña unas cuantas más de tus frases más largas.

journal.window.shared.gate.drawer.window.explanation.voice.pattern.is.still.too.spread = el patrón de voz todavía está demasiado disperso

journal.window.shared.gate.drawer.window.explanation.you.ve.taught.enough.to.save = ya enseñaste lo suficiente para guardar tu voz.

journal.window.shared.import.item.dated.label = { $label } · { $date }

journal.window.shared.import.item.kind.conversation = conversación

journal.window.shared.import.item.kind.document = documento

journal.window.shared.import.item.kind.messages = mensajes

journal.window.shared.import.item.kind.note = nota

journal.window.shared.import.item.kind.photo = foto

journal.window.shared.import.item.kind.screenshot = captura de pantalla

journal.window.shared.import.item.kind.transcript = transcripción

journal.window.shared.import.item.label = { $kind } · { $source }

journal.window.shared.import.item.source.chatgpt = ChatGPT

journal.window.shared.import.item.source.claude = Claude

journal.window.shared.import.item.source.evernote = Evernote

journal.window.shared.import.item.source.gemini = Gemini

journal.window.shared.import.item.source.imported = importado

journal.window.shared.import.item.source.obsidian = Obsidian

journal.window.shared.import.item.source.pdf = PDF

journal.window.shared.import.item.text.unavailable = no hay texto disponible

journal.window.shared.mount.workspace.button.label.try.again = reintentar

journal.window.shared.mount.workspace.section.heading.couldn.t.load.this.section = no se pudo cargar esta sección

journal.window.shared.mount.workspace.visible.text.reload.to.try.again = recarga para volver a intentarlo.

journal.window.shared.question.visible.text.cancel = cancelar

journal.window.shared.question.visible.text.name = nombre

journal.window.shared.report.error.journal.version = diario{ $slot1 }{ $version }

journal.window.shared.report.error.status.text.your.versions = tus versiones

journal.window.shared.report.error.window.explanation.nothing.leaves.this.journal.until.you = nada sale de este diario hasta que envíes el formulario en el sitio de soporte.

journal.window.shared.shell.boot.accessible.name.about = acerca de

journal.window.shared.shell.boot.accessible.name.connecting = conectando

journal.window.shared.shell.boot.button.label.try.again = reintentar

journal.window.shared.shell.boot.section.heading.couldn.t.load.this.section = no se pudo cargar esta sección

journal.window.shared.shell.boot.tooltip.connecting = conectando

journal.window.shared.shell.boot.visible.text.more = más

journal.window.shared.shell.boot.visible.text.reload.to.try.again = recarga para volver a intentarlo.

journal.window.shared.shell.boot.window.explanation.no.system.messages.this.session = no hay mensajes del sistema en esta sesión.

journal.window.shared.shell.boot.window.explanation.sending.reports.is.off = el envío de reportes está desactivado.

journal.window.shared.status.pane.accessible.label.browser.notifications.enabled = notificaciones del navegador activadas

journal.window.shared.status.pane.accessible.label.enable.browser.notifications = activar notificaciones del navegador

journal.window.shared.status.pane.accessible.label.notifications.blocked.update.in.browser.settings = notificaciones bloqueadas. actualiza en la configuración del navegador

journal.window.shared.status.pane.age.elapsed.day = { $quantity ->
    [one] hace { $count } día
    [many] hace { $count } días
   *[other] hace { $count } días
}

journal.window.shared.status.pane.age.elapsed.hour = { $quantity ->
    [one] hace { $count } hora
    [many] hace { $count } horas
   *[other] hace { $count } horas
}

journal.window.shared.status.pane.age.elapsed.minute = { $quantity ->
    [one] hace { $count } minuto
    [many] hace { $count } minutos
   *[other] hace { $count } minutos
}

journal.window.shared.status.pane.age.elapsed.month = { $quantity ->
    [one] hace { $count } mes
    [many] hace { $count } meses
   *[other] hace { $count } meses
}

journal.window.shared.status.pane.age.elapsed.second = { $quantity ->
    [one] hace { $count } segundo
    [many] hace { $count } segundos
   *[other] hace { $count } segundos
}

journal.window.shared.status.pane.age.elapsed.week = { $quantity ->
    [one] hace { $count } semana
    [many] hace { $count } semanas
   *[other] hace { $count } semanas
}

journal.window.shared.status.pane.automatic.decline = no, gracias

journal.window.shared.status.pane.automatic.enable = activar

journal.window.shared.status.pane.automatic.held = las actualizaciones están retenidas

journal.window.shared.status.pane.automatic.installs = se instala automáticamente

journal.window.shared.status.pane.automatic.paused = las actualizaciones automáticas se detuvieron tras dos intentos que no funcionaron.

journal.window.shared.status.pane.automatic.retry = reintentar

journal.window.shared.status.pane.automatic.updates = se actualiza automáticamente

journal.window.shared.status.pane.capture.device.attention = { $device } necesita atención

journal.window.shared.status.pane.capture.last.reached.day = lo que agrega no ha llegado a tu diario desde { $since_day }.

journal.window.shared.status.pane.capture.more.need.attention = { $quantity ->
        [one] y { $count } más necesita atención
        [many] y { $count } más necesitan atención
       *[other] y { $count } más necesitan atención
    }

journal.window.shared.status.pane.capture.rejected.upload.count = { $quantity ->
        [one] 1 subida rechazada.
        [many] { $count } subidas rechazadas.
       *[other] { $count } subidas rechazadas.
    }

journal.window.shared.status.pane.connected.elapsed.day = { $quantity ->
        [one] conectado durante { $count } día
        [many] conectado durante { $count } días
       *[other] conectado durante { $count } días
    }

journal.window.shared.status.pane.connected.elapsed.hour = { $quantity ->
        [one] conectado durante { $count } hora
        [many] conectado durante { $count } horas
       *[other] conectado durante { $count } horas
    }

journal.window.shared.status.pane.connected.elapsed.minute = { $quantity ->
        [one] conectado durante { $count } minuto
        [many] conectado durante { $count } minutos
       *[other] conectado durante { $count } minutos
    }

journal.window.shared.status.pane.connected.elapsed.month = { $quantity ->
        [one] conectado durante { $count } mes
        [many] conectado durante { $count } meses
       *[other] conectado durante { $count } meses
    }

journal.window.shared.status.pane.connected.elapsed.second = { $quantity ->
        [one] conectado durante { $count } segundo
        [many] conectado durante { $count } segundos
       *[other] conectado durante { $count } segundos
    }

journal.window.shared.status.pane.connected.elapsed.week = { $quantity ->
        [one] conectado durante { $count } semana
        [many] conectado durante { $count } semanas
       *[other] conectado durante { $count } semanas
    }

journal.window.shared.status.pane.connection.just.now = conectado justo ahora

journal.window.shared.status.pane.rejected.elapsed.day = { $quantity ->
        [one] último rechazo hace { $count } día
        [many] último rechazo hace { $count } días
       *[other] último rechazo hace { $count } días
    }

journal.window.shared.status.pane.rejected.elapsed.hour = { $quantity ->
        [one] último rechazo hace { $count } hora
        [many] último rechazo hace { $count } horas
       *[other] último rechazo hace { $count } horas
    }

journal.window.shared.status.pane.rejected.elapsed.minute = { $quantity ->
        [one] último rechazo hace { $count } minuto
        [many] último rechazo hace { $count } minutos
       *[other] último rechazo hace { $count } minutos
    }

journal.window.shared.status.pane.rejected.elapsed.month = { $quantity ->
        [one] último rechazo hace { $count } mes
        [many] último rechazo hace { $count } meses
       *[other] último rechazo hace { $count } meses
    }

journal.window.shared.status.pane.rejected.elapsed.second = { $quantity ->
        [one] último rechazo hace { $count } segundo
        [many] último rechazo hace { $count } segundos
       *[other] último rechazo hace { $count } segundos
    }

journal.window.shared.status.pane.rejected.elapsed.week = { $quantity ->
        [one] último rechazo hace { $count } semana
        [many] último rechazo hace { $count } semanas
       *[other] último rechazo hace { $count } semanas
    }

journal.window.shared.status.pane.security.device.update.version = a la app de solstone en { $device_name } le falta una corrección de seguridad (actualiza a { $fixed_version }).

journal.window.shared.status.pane.security.journal.update.version = a tu diario le falta una corrección de seguridad (actualiza a { $fixed_version }).

journal.window.shared.status.pane.security.restart = reiniciar tu diario

journal.window.shared.status.pane.security.unnamed.device.update.version = a la app de solstone en uno de tus dispositivos le falta una corrección de seguridad (actualiza a { $fixed_version }).

journal.window.shared.status.pane.status.text.a.device.isn.t.reaching.your = un dispositivo no conecta con tu diario.

journal.window.shared.status.pane.status.text.a.device.needs.attention = un dispositivo necesita atención

journal.window.shared.status.pane.status.text.update.or.restart.it.on.that = actualízalo o reinícialo en ese dispositivo y, después, una subida válida limpia esto.

journal.window.shared.status.pane.tooltip.browser.notifications.enabled = notificaciones del navegador activadas

journal.window.shared.status.pane.tooltip.enable.browser.notifications = activar notificaciones del navegador

journal.window.shared.status.pane.tooltip.notifications.blocked.update.in.browser.settings = notificaciones bloqueadas. actualiza en la configuración del navegador

journal.window.shared.status.pane.update.just.now = actualizado justo ahora

journal.window.shared.status.pane.updated.elapsed.day = { $quantity ->
        [one] actualizado hace { $count } día
        [many] actualizado hace { $count } días
       *[other] actualizado hace { $count } días
    }

journal.window.shared.status.pane.updated.elapsed.hour = { $quantity ->
        [one] actualizado hace { $count } hora
        [many] actualizado hace { $count } horas
       *[other] actualizado hace { $count } horas
    }

journal.window.shared.status.pane.updated.elapsed.minute = { $quantity ->
        [one] actualizado hace { $count } minuto
        [many] actualizado hace { $count } minutos
       *[other] actualizado hace { $count } minutos
    }

journal.window.shared.status.pane.updated.elapsed.month = { $quantity ->
        [one] actualizado hace { $count } mes
        [many] actualizado hace { $count } meses
       *[other] actualizado hace { $count } meses
    }

journal.window.shared.status.pane.updated.elapsed.second = { $quantity ->
        [one] se actualizó hace { $count } segundo
        [many] se actualizó hace { $count } segundos
       *[other] se actualizó hace { $count } segundos
    }

journal.window.shared.status.pane.updated.elapsed.week = { $quantity ->
        [one] se actualizó hace { $count } semana
        [many] se actualizó hace { $count } semanas
       *[other] se actualizó hace { $count } semanas
    }

journal.window.shared.status.pane.version.current.up.to.date = v{ $current_version } · al día

journal.window.shared.status.pane.version.package.repository.link = instalado desde un paquete. para recibir actualizaciones con el actualizador de tu sistema, agrega el repositorio de solstone: { $repository_link }

journal.window.shared.status.pane.version.unknown.up.to.date = vunknown · al día

journal.window.shared.status.pane.version.update.available = v{ $current_version } · { $notice_start }actualización disponible (v{ $available_version }){ $notice_end }

journal.window.shared.status.pane.version.update.installed = v{ $current_version } · v{ $installed_version } está instalada y se inicia la próxima vez que se inicie tu diario

journal.window.shared.status.pane.visible.text.a.security.update.is.installed.restart = una actualización de seguridad está instalada. reinicia tu diario para terminarla.

journal.window.shared.status.pane.visible.text.couldn.t.check.for.security.updates = no se pudo revisar si hay actualizaciones de seguridad.

journal.window.shared.status.pane.visible.text.device.status.is.unavailable.right.now = el estado del dispositivo no está disponible ahora.

journal.window.shared.status.pane.visible.text.devices.are.adding.to.your.journal = los dispositivos están agregando a tu diario

journal.window.shared.status.pane.visible.text.hide.details = ocultar detalles

journal.window.shared.status.pane.visible.text.just.now = justo ahora

journal.window.shared.status.pane.visible.text.no.devices.are.running.the.solstone = todavía no hay dispositivos ejecutando la app de solstone. configura uno para empezar tu diario.

journal.window.shared.status.pane.visible.text.no.messages.yet = todavía no hay mensajes

journal.window.shared.status.pane.visible.text.no.notifications.held.back = no hay notificaciones retenidas

journal.window.shared.status.pane.visible.text.no.notifications.yet = todavía no hay notificaciones

journal.window.shared.status.pane.visible.text.nothing.held.back.nothing.new = nada retenido, nada nuevo

journal.window.shared.status.pane.visible.text.notifications = notificaciones

journal.window.shared.status.pane.visible.text.quiet.notifications = notificaciones silenciosas

journal.window.shared.status.pane.visible.text.show.details = mostrar detalles

journal.window.shared.status.pane.visible.text.solstone.app.install.linux.packages = solstone.app/install#linux-packages

journal.window.shared.status.pane.visible.text.under.a.minute = menos de un minuto

journal.window.shared.status.pane.visible.text.unknown = desconocido

journal.window.shared.status.pane.visible.text.update.status.unknown = estado de la actualización desconocido

journal.window.shared.status.pane.visible.text.view.device.health = ver la salud del dispositivo →

journal.window.shared.status.pane.window.explanation.a.device.hasn.t.added.to = un dispositivo no ha agregado a tu diario recientemente.

journal.window.shared.status.pane.window.explanation.a.device.needs.attention = un dispositivo necesita atención

journal.window.shared.status.pane.window.explanation.installed.from.a.package.updates.come = instalado desde un paquete. las actualizaciones llegan por el actualizador de tu sistema.

journal.window.shared.status.pane.window.explanation.installs.on.a.connection.that.isn = se instala en una conexión que no es de uso medido

journal.window.shared.status.pane.window.explanation.once.a.day.your.journal.and = una vez al día, tu diario y la app de solstone revisan si hay una versión nueva y la instalan en segundo plano. tu diario nunca se reinicia solo.

journal.window.shared.status.pane.window.explanation.once.a.day.your.journal.checks = una vez al día, tu diario revisa si hay una versión nueva y la instala en segundo plano. nunca se reinicia solo.

journal.window.shared.status.pane.window.explanation.update.your.journal.automatically = ¿actualizar tu diario automáticamente?

journal.window.shared.status.pane.window.explanation.what.it.adds.isn.t.reaching = lo que agrega no está llegando a tu diario.

journal.window.shared.websocket.label.a.device.hasn.t.added.recently = un dispositivo no ha agregado recientemente

journal.window.shared.websocket.label.a.device.needs.attention = un dispositivo necesita atención

journal.window.shared.websocket.label.attention = atención

journal.window.shared.websocket.label.can.t.confirm = no se puede confirmar

journal.window.shared.websocket.label.connecting = conectando

journal.window.shared.websocket.label.disconnected = desconectado

journal.window.shared.websocket.label.going.in = entrando

journal.window.shared.websocket.label.no.devices.connected = no hay dispositivos conectados

journal.window.shared.websocket.label.reconnect.now = Reconectar ahora

journal.window.shared.websocket.label.show.details = Mostrar detalles

journal.window.shared.websocket.message.last.reconnect.attempt.failed = falló el último intento de reconexión.

journal.window.shared.websocket.message.live.updates.connected = actualizaciones en vivo conectadas

journal.window.shared.websocket.message.reconnecting.some.features.may.be.delayed = reconectando. algunas funciones pueden demorarse

journal.window.shared.websocket.reconnect.failed.history = el último intento de reconexión falló. intentos recientes: { $attempt_times }

journal.window.shared.websocket.reconnect.failed.no.history = el último intento de reconexión falló. todavía no hay intentos de reconexión registrados.

journal.window.shared.websocket.title.connection.lost = se perdió la conexión

journal.window.shared.websocket.title.reconnected = se reconectó

journal.window.shell.connecting = conectando…

journal.window.shell.error.pinned.must.be.a.boolean = anclado tiene que ser un booleano

journal.window.shell.error.pins.request.must.be.an.object = la solicitud de anclajes tiene que ser un objeto

journal.window.shell.error.that.view.can.t.be.pinned = no se puede anclar esa vista

journal.window.shell.error.the.pins.request.could.not.be.read = no se pudo leer la solicitud de anclajes

journal.window.shell.error.unknown.view = vista desconocida

journal.window.shell.error.view.must.be.a.string = la vista tiene que ser una cadena

journal.window.shell.error.your.settings.couldn.t.be.read = no se pudo leer tu configuración.

journal.window.shell.guard.authorization.revoked = no se pudo usar ese dispositivo vinculado porque se revocó.

journal.window.shell.guard.authorization.revoked.detail = dispositivo vinculado revocado

journal.window.shell.guard.authorization.unavailable.detail = la autorización del dispositivo vinculado no está disponible

journal.window.shell.guard.authorization.unavailable.mac = no se pudo revisar qué dispositivos están vinculados con tu diario. si sigue pasando, abre la terminal de administración desde el menú journal de la app del diario, en la mac donde se ejecuta tu diario, y ejecuta `{ $slot1 }`.

journal.window.shell.guard.authorization.unavailable.other = no se pudo revisar qué dispositivos están vinculados con tu diario. si sigue pasando, ejecuta `{ $slot1 }` en la computadora donde se ejecuta tu diario.

journal.window.shell.guard.form.denied = tu diario rechazó esta solicitud porque no la enviaron sus propias páginas.

journal.window.shell.guard.form.detail = las solicitudes que cambian el estado aceptan { $slot1 }

journal.window.shell.guard.host.denied = la app web de tu diario solo se puede abrir en { $slot1 }, { $slot2 } o { $slot3 }, en la computadora donde se ejecuta.

journal.window.shell.guard.host.detail = host no permitido

journal.window.shell.guard.origin.detail = solicitud de otro origen

journal.window.shell.guard.site.denied = la app web de tu diario rechazó esta solicitud porque vino de otro sitio web.

journal.window.shell.guard.site.detail = solicitud de otro sitio

journal.window.shell.last.message = último mensaje:

journal.window.shell.loading = cargando…

journal.window.shell.notifications.empty = todavía no hay notificaciones

journal.window.shell.notifications.enable = activar notificaciones del navegador

journal.window.shell.notifications.label = notificaciones

journal.window.shell.notifications.quiet = notificaciones silenciosas

journal.window.shell.notifications.quiet.explanation = errores de servicios en segundo plano que no se mostraron como notificaciones.

journal.window.shell.notifications.recent = notificaciones recientes

journal.window.shell.pin.error.invalid = no se pudo guardar esa configuración porque un valor no era válido.

journal.window.shell.pin.error.save = no se pudo guardar tu configuración.

journal.window.shell.presentation.banner = PRESENTACIÓN — Shift+P para salir

journal.window.shell.redirect.instruction = Deberías llegar automáticamente a la URL de destino: { $destination_link }. Si no, haz clic en el enlace.

journal.window.shell.redirect.title = Redirigiendo...

journal.window.shell.settings.read.failed = no se pudo leer tu configuración.

journal.window.shell.skip.to.workspace = saltar al espacio de trabajo

journal.window.shell.state = estado:

journal.window.shell.status = estado

journal.window.shell.status.details = detalles del estado del diario

journal.window.shell.status.silent.devices = { $count }{ $slot1 }dispositivos no han agregado recientemente

journal.window.shell.technical.details = detalles técnicos

journal.window.shell.title = journal

journal.window.shell.update.link = cómo actualizar tu diario

journal.window.shell.uptime = tiempo activo:

journal.window.shell.version = versión

journal.window.shell.version.unknown = vunknown

journal.window.shell.views = vistas

journal.window.view.activities.date.count = { $quantity ->
        [one] 1 actividad
        [many] { $count } actividades
       *[other] { $count } actividades
    }

journal.window.view.activities.date.none = sin actividades

journal.window.view.activities.date.one = actividad

journal.window.view.activities.date.other = actividades

journal.window.view.activities.description = lo que hiciste cada día, ordenado por faceta

journal.window.view.activities.label = actividades

journal.window.view.activities.source = redactado por tu diario a partir de lo que compartiste

journal.window.view.agents.description = conecta un agente para leer y buscar en tu diario, dentro de lo que eliges

journal.window.view.agents.label = agentes

journal.window.view.agents.source = agentes que conectas

journal.window.view.backup.description = una copia cifrada de tu diario que solo tú puedes leer

journal.window.view.backup.label = copia de seguridad

journal.window.view.backup.source = tu propio almacenamiento, o almacenamiento que opera { $slot1 }

journal.window.view.body.date.count = { $quantity ->
        [one] 1 lectura
        [many] { $count } lecturas
       *[other] { $count } lecturas
    }

journal.window.view.body.date.none = sin lecturas

journal.window.view.body.date.one = lectura

journal.window.view.body.date.other = lecturas

journal.window.view.body.description = sueño, entrenamientos y lecturas del corazón

journal.window.view.body.label = cuerpo

journal.window.view.body.source = de lo que importas

journal.window.view.curation.description = cambios que tu diario sugiere, para que los aceptes o los omitas

journal.window.view.curation.label = sugerencias

journal.window.view.curation.source = sugerido por tu diario mientras trabaja

journal.window.view.entities.description = las personas, empresas, proyectos y herramientas que tu diario guarda, para editar, fusionar u ocultar

journal.window.view.entities.label = entidades

journal.window.view.entities.source = nombres que tu diario encontró en lo que guarda

journal.window.view.health.description = si tu diario va al día: procesamiento, dispositivos, errores

journal.window.view.health.label = estado

journal.window.view.health.source = las revisiones propias de tu diario

journal.window.view.home.description = hoy y ayer, redactados, y lo que tu diario necesita de ti

journal.window.view.home.label = inicio

journal.window.view.home.source = escrito por tu diario a partir de todo lo que hay en él

journal.window.view.import.description = trae chats, calendarios, notas, audio y entrenamientos

journal.window.view.import.label = importación

journal.window.view.import.source = archivos que traes

journal.window.view.map.date.count = { $quantity ->
        [one] 1 registro
        [many] { $count } registros
       *[other] { $count } registros
    }

journal.window.view.map.date.none = sin ubicación

journal.window.view.map.date.one = registro

journal.window.view.map.date.other = registros

journal.window.view.map.description = dónde estuviste, día a día, y los lugares que nombras

journal.window.view.map.label = mapa

journal.window.view.map.source = desde la app de solstone en tu teléfono, cuando la ubicación está activada ahí

journal.window.view.more.label = más

journal.window.view.network.description = vincula un teléfono o una computadora, y cómo llegan tus dispositivos a tu diario

journal.window.view.network.label = red

journal.window.view.network.source = tus dispositivos

journal.window.view.search.description = busca palabras y frases en cualquier parte de tu diario

journal.window.view.search.label = búsqueda

journal.window.view.search.source = todo lo que tu diario guarda

journal.window.view.settings.description = tu perfil, claves, transcripción, almacenamiento y facetas

journal.window.view.settings.label = configuración

journal.window.view.sources.description = cuándo tus dispositivos agregaron algo a tu diario por última vez.

journal.window.view.sources.label = fuentes

journal.window.view.sources.source = tus dispositivos vinculados

journal.window.view.speakers.date.count = { $quantity ->
        [one] 1 segmento
        [many] { $count } segmentos
       *[other] { $count } segmentos
    }

journal.window.view.speakers.date.none = sin segmentos

journal.window.view.speakers.date.one = segmento

journal.window.view.speakers.date.other = segmentos

journal.window.view.speakers.description = las voces de tu audio: enseña la tuya, nombra a las demás, corrige quién dijo qué

journal.window.view.speakers.label = hablantes

journal.window.view.speakers.source = voces en el audio que compartiste

journal.window.view.stats.description = cuánto guarda tu diario, y cuánto procesamiento ha hecho

journal.window.view.stats.label = estadísticas

journal.window.view.stats.source = los conteos propios de tu diario

journal.window.view.support.description = haz un informe, o sigue uno que hiciste

journal.window.view.support.label = soporte

journal.window.view.thinking.description = dónde ocurre el razonamiento, y cada ejecución

journal.window.view.thinking.label = razonamiento

journal.window.view.thinking.source = en la computadora de tu diario, en procesamiento confidencial operado por { $slot1 }, o con tu propio modelo

journal.window.view.transcripts.date.count = { $quantity ->
        [one] 1 segmento
        [many] { $count } segmentos
       *[other] { $count } segmentos
    }

journal.window.view.transcripts.date.none = sin segmentos

journal.window.view.transcripts.date.one = segmento

journal.window.view.transcripts.date.other = segmentos

journal.window.view.transcripts.description = lo que se dijo y lo que hubo en pantalla, momento a momento, con el audio

journal.window.view.transcripts.label = transcripciones

journal.window.view.transcripts.source = desde tu micrófono y tu pantalla, por la app de solstone, y desde importaciones

journal.window.view.year.description = un año entero de una vez: cada día, y cada lugar y nombre por semana

journal.window.view.year.label = año

journal.window.view.year.source = se cuenta a partir de lo que tu diario guarda

journal.window.year.copy.days.days = días

journal.window.year.copy.days.lede.each.square.is.a.day.colored = cada cuadrado es un día, coloreado según el conteo que eliges abajo.

journal.window.year.copy.empty.action.see.your.sources = ver tus fuentes

journal.window.year.copy.empty.detail.once.your.journal.has.activities.places = cuando tu diario tenga actividades, lugares o nombres para un día, su año se muestra aquí.

journal.window.year.copy.empty.nothing.here.yet = todavía no hay nada aquí.

journal.window.year.copy.error.this.didn.t.load = esto no se cargó.

journal.window.year.copy.find.find.a.place.or.a.name = buscar un lugar o un nombre

journal.window.year.copy.layer.label.what.to.count = qué contar

journal.window.year.copy.names.unavailable.names.aren.t.listed.your.journal = no se listan los nombres: no se pudo revisar cuáles dejar fuera.

journal.window.year.copy.other.names.other.names = otros nombres

journal.window.year.copy.peek.hint.choose.a.row.or.a.week = elige una fila, o una semana en ella, para ver sus días.

journal.window.year.copy.places.places = lugares

journal.window.year.copy.places.unavailable.places.aren.t.listed.the.place = no se listan los lugares: no se pudieron leer los nombres de lugar que vienen con tu diario.

journal.window.year.copy.retry.try.again = reintentar

journal.window.year.copy.step.label.other.years = otros años

journal.window.year.copy.switch.label.how.to.see.this.year = cómo ver este año

journal.window.year.copy.threads.lede.each.row.is.a.place.or = cada fila es un lugar o un nombre que encontró tu diario. cada cuadrado es una semana, coloreada por cuántos días de esa semana aparece el lugar o el nombre. las filas van en el orden de su primera aparición.

journal.window.year.copy.threads.threads = hilos

journal.window.year.copy.title.year = año

journal.window.year.copy.week.after.the.later.week.with.days.on = la semana posterior con días en esta fila

journal.window.year.copy.week.before.the.earlier.week.with.days.on = la semana anterior con días en esta fila

journal.window.year.days.empty = nada de este tipo en { $year }.

journal.window.year.months.label.apr = abr

journal.window.year.months.label.aug = ago

journal.window.year.months.label.dec = dic

journal.window.year.months.label.feb = feb

journal.window.year.months.label.jan = ene

journal.window.year.months.label.jul = jul

journal.window.year.months.label.jun = jun

journal.window.year.months.label.mar = mar

journal.window.year.months.label.may = may

journal.window.year.months.label.nov = nov

journal.window.year.months.label.oct = oct

journal.window.year.months.label.sep = sep

journal.window.year.source.or.date.unit.label.activities = actividades

journal.window.year.source.or.date.unit.label.activity = actividad

journal.window.year.source.or.date.unit.label.location = ubicación

journal.window.year.source.or.date.unit.label.no.activities = sin actividades

journal.window.year.source.or.date.unit.label.no.location = sin ubicación

journal.window.year.source.or.date.unit.label.record = registro

journal.window.year.source.or.date.unit.label.records = registros

journal.window.year.thread.cell.week.days = { $days_quantity ->
        [one] { $thread_name }, semana de { $week_start }: { $days_count } día
        [many] { $thread_name }, semana de { $week_start }: { $days_count } días
       *[other] { $thread_name }, semana de { $week_start }: { $days_count } días
    }

journal.window.year.thread.days.in.year = { $days_quantity ->
    [one] 1 día en { $year }, { $first_day }.
    [many] { $days_count } días en { $year }, de { $first_day } a { $last_day }, a lo largo de { $weeks_quantity ->
        [one] { $weeks_count } semana.
        [many] { $weeks_count } semanas.
       *[other] { $weeks_count } semanas.
    }
   *[other] { $days_count } días en { $year }, de { $first_day } a { $last_day }, a lo largo de { $weeks_quantity ->
        [one] { $weeks_count } semana.
        [many] { $weeks_count } semanas.
       *[other] { $weeks_count } semanas.
    }
}

journal.window.year.thread.empty = no hay lugares ni nombres en { $year }.

journal.window.year.thread.find.empty = nada en { $year } tiene ese nombre.

journal.window.year.thread.group.name.count = { $total_quantity ->
        [one] { $total_count } nombre
        [many] { $total_count } nombres
       *[other] { $total_count } nombres
    }

journal.window.year.thread.group.place.count = { $total_quantity ->
        [one] { $total_count } lugar
        [many] { $total_count } lugares
       *[other] { $total_count } lugares
    }

journal.window.year.thread.jump.to.first.day = empezar en { $first_day }

journal.window.year.thread.list.empty = nada listado para { $year }.

journal.window.year.thread.week.day.links = { $days_quantity ->
        [one] { $days_count } día de esta semana: { $day_links }
        [many] { $days_count } días de esta semana: { $day_links }
       *[other] { $days_count } días de esta semana: { $day_links }
    }

journal.window.year.thread.week.range = semana de { $start_day } a { $end_day }

journal.window.year.type.labels.company.companies = empresas

journal.window.year.type.labels.organization.organizations = organizaciones

journal.window.year.type.labels.person.people = personas

journal.window.year.type.labels.place.named.places = lugares con nombre

journal.window.year.type.labels.project.projects = proyectos

journal.window.year.type.labels.tool.tools = herramientas

journal.window.year.view.empty = nada que mostrar aquí para { $year }.

journal.window.year.year.invalid = no se pudo usar ese año.

journal.window.year.year.invalid.detail = Año no válido, se espera YYYY
