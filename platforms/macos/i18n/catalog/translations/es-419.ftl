journal.mac.admin.terminal.failure.title = no se pudo abrir la terminal de administración

journal.mac.admin.terminal.menu.item.title = abrir la terminal de administración

journal.mac.app.journal = journal

journal.mac.backup.setup.hint = la copia de seguridad mantiene tu diario a salvo. configúrala desde tu diario.

journal.mac.connection.invalid.response = la app del diario no pudo leer lo que volvió de tu diario.

journal.mac.connection.link.failed = la app del diario no pudo abrir tu diario en tu navegador. vuelve a intentarlo.

journal.mac.connection.refused = la app del diario no pudo conectarse a tu diario en esta mac.

journal.mac.connection.timed.out = la app del diario esperó demasiado a tu diario. vuelve a intentarlo.

journal.mac.connection.unavailable = tu diario todavía no está listo para la app del diario. vuelve a intentarlo en un momento.

journal.mac.devices.add.device = vincular un dispositivo

journal.mac.devices.cancel = cancelar

journal.mac.devices.close = cerrar

journal.mac.devices.copied = copiado ✓

journal.mac.devices.copy.link = copiar enlace

journal.mac.devices.detail.network = { $network } · { $activity }

journal.mac.devices.detail.network.paired = { $network } · { $activity } · vinculado { $date }

journal.mac.devices.detail.paired = { $activity } · vinculado { $date }

journal.mac.devices.empty.body = vincula un dispositivo y se une a tu diario.

journal.mac.devices.empty.title = todavía no hay dispositivos

journal.mac.devices.expires.in = vence en { $minutes }:{ $seconds }

journal.mac.devices.link.expired.body = abre un enlace nuevo para vincular un dispositivo.

journal.mac.devices.link.expired.title = enlace vencido

journal.mac.devices.loading = cargando dispositivos...

journal.mac.devices.model.last.seen = visto por última vez { $time }

journal.mac.devices.model.last.seen.just.now = visto por última vez justo ahora

journal.mac.devices.model.last.seen.unknown = visto por última vez desconocido

journal.mac.devices.model.never.connected = nunca se conectó

journal.mac.devices.model.status = estado { $status }

journal.mac.devices.not.ready.body = termina de configurar tu diario y vuelve a intentarlo.

journal.mac.devices.not.ready.title = los dispositivos no están listos

journal.mac.devices.not.running.body = inicia el diario y vuelve a intentarlo.

journal.mac.devices.not.running.title = el diario no está en ejecución

journal.mac.devices.open.fresh.link = abrir un enlace nuevo

journal.mac.devices.paired.done = listo

journal.mac.devices.paired.heading = "{ $label }" ya puede conectar con tu diario

journal.mac.devices.paired.heading.unnamed = un dispositivo nuevo ya puede conectar con tu diario

journal.mac.devices.paired.note = en ese dispositivo, revisa que muestre esta misma marca y responde ahí. ¿no lo vinculaste?

journal.mac.devices.paired.note.anywhere = en ese dispositivo, revisa que muestre esta misma marca y responde ahí. puede conectar con tu diario desde cualquier lugar. ¿no lo vinculaste?

journal.mac.devices.paired.remove.failed = no se pudo desvincular este dispositivo. vuelve a intentarlo, o quítalo en dispositivos.

journal.mac.devices.paired.removed = desvinculado. el dispositivo no puede conectar con tu diario hasta que se vuelva a vincular.

journal.mac.devices.paired.unpair = desvincular este dispositivo

journal.mac.devices.pairing.code = código de vinculación

journal.mac.devices.pairing.failed.title = no se pudo abrir la vinculación

journal.mac.devices.pairing.instructions = escanea este código o abre el enlace en el dispositivo que quieres vincular.

journal.mac.devices.pairing.link.label = enlace de vinculación

journal.mac.devices.pairing.opening = abriendo la vinculación...

journal.mac.devices.pairing.title = vincular un dispositivo

journal.mac.devices.pane.title = dispositivos

journal.mac.devices.peer.journals.header = diarios pares

journal.mac.devices.remove = quitar

journal.mac.devices.revoke.body = { $name } pierde el acceso a tu diario. puedes volver a vincularlo más tarde.{ $space1 }{ $detail }

journal.mac.devices.revoke.failed = no se pudo quitar este dispositivo

journal.mac.devices.revoke.title = ¿quitar { $name }?

journal.mac.devices.try.again = reintentar

journal.mac.devices.unnamed.device = dispositivo sin nombre

journal.mac.devices.unnamed.journal = diario sin nombre

journal.mac.devices.your.devices.header = tus dispositivos

journal.mac.events.completed = configuración inicial ok

journal.mac.events.completed.duration = configuración inicial ok ({ $duration_ms }ms)

journal.mac.events.doctor.check = { $space0 }[{ $label }] { $name } - { $detail }

journal.mac.events.doctor.started = doctor iniciado

journal.mac.events.doctor.status = doctor: { $status }

journal.mac.events.doctor.summary = { $total ->
    [one] doctor: { $total } revisión ({ $failed } fallos, { $warnings } avisos, { $skipped } omisiones)
    [many] doctor: { $total } revisiones ({ $failed } fallos, { $warnings } avisos, { $skipped } omisiones)
   *[other] doctor: { $total } revisiones ({ $failed } fallos, { $warnings } avisos, { $skipped } omisiones)
}

journal.mac.events.done = paso { $step } listo ({ $outcome })

journal.mac.events.done.duration = paso { $step } listo ({ $outcome }, { $duration_ms }ms)

journal.mac.events.fail = fallo

journal.mac.events.failed = la configuración inicial falló

journal.mac.events.failed.step = la configuración inicial falló en { $failed_step }

journal.mac.events.malformed = [evento malformado] { $raw }

journal.mac.events.ok = ok

journal.mac.events.skip = omisión

journal.mac.events.skipped = paso { $step } omitido

journal.mac.events.skipped.reason = paso { $step } omitido ({ $reason })

journal.mac.events.started = se inició la configuración inicial

journal.mac.events.started.version = se inició la configuración inicial (versión { $version }, { $mode })

journal.mac.events.step = paso: { $step }

journal.mac.events.step.failed = el paso { $step } falló: { $message }

journal.mac.events.step.indexed = paso { $index }/{ $total }: { $step }

journal.mac.events.unknown = desconocido

journal.mac.events.unparseable = [línea no analizable] { $raw }

journal.mac.events.unrecognized = [evento no reconocido] { $raw }

journal.mac.events.warn = aviso

journal.mac.events.warning = { $space0 }aviso: { $text }

journal.mac.events.warning.fix = { $space0 }aviso: { $text } (solución: { $fix_hint })

journal.mac.first.run.model.setup.did.not.finish = la configuración inicial no terminó

journal.mac.first.run.model.setup.reported.completion.twice = la configuración inicial informó la finalización dos veces

journal.mac.first.run.model.setup.timed.out = se agotó el tiempo de espera de la configuración inicial

journal.mac.health.check.denied = denegado

journal.mac.health.check.granted = concedido

journal.mac.health.check.microphone = micrófono

journal.mac.health.check.screen.recording = grabación de pantalla

journal.mac.health.check.unknown.checking.permissions = desconocido, revisando permisos

journal.mac.health.microphone.permission.help = abre Configuración del Sistema › Privacidad y seguridad y permite Micrófono para { $product }

journal.mac.health.screen.permission.help = abre Configuración del Sistema › Privacidad y seguridad y permite Grabación de pantalla y audio del sistema para { $product }

journal.mac.home.browser.explanation = esta app mantiene tu diario en ejecución en esta mac. tu diario en sí se abre en tu navegador.

journal.mac.home.unconfigured = todavía no hay nada aquí. lo siguiente es crear tu diario.

journal.mac.info.cfbundle.display.name = journal

journal.mac.info.cfbundle.name = journal

journal.mac.info.nshuman.readable.copyright = © 2026 { $company }. todos los derechos reservados.

journal.mac.local.endpoint.unavailable = tu diario todavía no está listo para la app del diario. vuelve a intentarlo en un momento.

journal.mac.location.cancel = cancelar

journal.mac.location.leave.setup.confirm = la configuración inicial se detiene.

journal.mac.location.new.journal.hosted.share = si restauras ahí la copia de seguridad de este diario, el diario nuevo se hace cargo de ella, y este deja de hacer la copia de seguridad hasta que la vuelvas a activar.

journal.mac.location.other.journals = otros diarios en esta mac

journal.mac.location.same.mark.template = ambos tienen la misma marca, así que tus dispositivos conectan con el que esté en ejecución.

journal.mac.location.shared.hosted.template = el que está en { $path } comparte esta copia de seguridad cifrada operada por { $company }, y solo uno puede usarla. si este diario deja de hacer la copia de seguridad, vuelve a activarla en el panel de copia de seguridad.

journal.mac.location.start.new.journal = iniciar un diario nuevo

journal.mac.location.start.new.journal.confirm.template = tu diario en { $path } se detiene y se queda ahí, sin cambios. después empieza la configuración inicial, donde puedes restaurar desde una copia de seguridad.

journal.mac.location.start.new.journal.confirm.title = ¿iniciar un diario nuevo?

journal.mac.location.start.new.journal.help = tu diario se detiene y se queda en su carpeta, sin cambios. durante la configuración inicial puedes restaurar desde una copia de seguridad, y puedes volver a este desde aquí.

journal.mac.location.switch.button = cambiar

journal.mac.location.switch.confirm.template = tu diario en { $path } se detiene y se queda ahí, sin cambios. puedes volver a él desde aquí.

journal.mac.location.switch.confirm.title.template = ¿cambiar al diario en { $path }?

journal.mac.location.switch.to.journal = cambiar a este diario

journal.mac.restart.journal.path.unreadable = el reinicio falló: no se pudo leer la ruta del diario

journal.mac.restart.restart.failed.journal.did.not.come.back = el reinicio falló: el diario no volvió

journal.mac.restart.restart.failed.journal.did.not.restart = el reinicio falló: el diario no se reinició

journal.mac.runtime.bundle.missing = falta el entorno de ejecución nativo empaquetado del diario en { $path }

journal.mac.runtime.child.admission.failed = no se pudo verificar la admisión del proceso hijo del diario

journal.mac.runtime.child.containment.pending = la contención del proceso hijo del diario está en curso

journal.mac.runtime.child.unconfigured = el proceso hijo del diario no está configurado

journal.mac.runtime.couldn.t.get.ready.continuing = no se pudo preparar { $library }; se continúa

journal.mac.runtime.executable.missing = falta el ejecutable nativo empaquetado de { $product } en { $path }

journal.mac.runtime.installer.readiness.gate.failed = no se pudo preparar el diario para esta mac

journal.mac.runtime.journal.child.breaker.tripped = tu diario se detuvo de forma inesperada varias veces en un minuto

journal.mac.runtime.journal.child.containment.unresolved = tu diario se detuvo, y no se pudo confirmar que todos sus procesos estuvieran cerrados

journal.mac.runtime.journal.drive.cannot.run = tu diario no puede ejecutarse en esta unidad porque la unidad no puede guardar archivos como tu diario lo necesita. usa una unidad con formato de mac para tu diario.

journal.mac.runtime.journal.materialize.failed = no se pudo preparar el entorno de ejecución del diario

journal.mac.runtime.journal.readiness.timeout = el diario no estuvo listo a tiempo

journal.mac.runtime.journal.setup.needed.before.upgrade = hace falta la configuración inicial del diario para que la actualización pueda continuar

journal.mac.runtime.journal.spawn.blocked.legacy.service = hay otra cosa configurada para ejecutar tu diario

journal.mac.runtime.journal.spawn.blocked.ports = los puertos del diario siguen en uso

journal.mac.runtime.journal.spawn.port.check.failed = no se pudo verificar que los puertos del diario estén libres

journal.mac.runtime.journal.spawn.port.held.elsewhere = tu diario no puede iniciarse porque otra app está usando la conexión que necesita en esta mac. en una mac compartida, esto suele ser el diario de otra cuenta.

journal.mac.runtime.journal.spawn.service.check.failed = no se pudo saber si otra cosa está ejecutando tu diario

journal.mac.runtime.release.identity.missing = a journal.app le falta una identidad de publicación válida

journal.mac.runtime.required.models.failed = { $command } falló (salida { $exit_code }, terminación { $reason })

journal.mac.settings.backup = copia de seguridad

journal.mac.settings.copied = copiado

journal.mac.settings.copy = copiar

journal.mac.settings.couldn.t.copy.select.the.text.and.copy.it = no se pudo copiar. selecciona el texto y cópialo.

journal.mac.settings.disk.used = disco usado

journal.mac.settings.health = estado

journal.mac.settings.launch.the.journal.when.you.log.in = inicia el diario cuando inicias sesión

journal.mac.settings.location = ubicación

journal.mac.settings.open.backup = abrir copia de seguridad

journal.mac.settings.open.your.journal = abrir tu diario

journal.mac.settings.restart = reiniciar

journal.mac.settings.run.state = estado de ejecución

journal.mac.settings.run.state.link = estado de ejecución →

journal.mac.settings.start = iniciar

journal.mac.settings.startup = arranque

journal.mac.settings.stop = detener

journal.mac.settings.the.journal.app.is.available = la app del diario { $version } está disponible.

journal.mac.settings.the.journal.update.check.failed = falló la búsqueda de actualizaciones del diario.

journal.mac.settings.update.failed.with.version = falló la búsqueda de actualizaciones. la app del diario { $version } se encontró antes.

journal.mac.settings.update.needs.attention = la actualización necesita atención

journal.mac.settings.view.journal.updates = ver actualizaciones del diario

journal.mac.settings.your.journal.at.a.glance = tu diario, de un vistazo

journal.mac.settings.your.journal.mark.not.read.yet = tu diario, marca todavía sin leer

journal.mac.setup.adopt.failed = no se pudo abrir este diario

journal.mac.setup.adopt.landing.line = no se movió nada. tu diario siempre estuvo aquí. lo reconoces por su marca.

journal.mac.setup.adopt.opening = abriendo este diario

journal.mac.setup.adopt.title = diario encontrado

journal.mac.setup.back = atrás

journal.mac.setup.choose.location = elegir

journal.mac.setup.chrome.of = { $done } de { $total }

journal.mac.setup.continue.button = continuar

journal.mac.setup.continue.in.the.solstone.app = continuar en la app de { $product }

journal.mac.setup.download.bytes = { $done } / { $total }

journal.mac.setup.finish.in.browser.line = esta ventana continúa cuando terminas.

journal.mac.setup.finish.in.browser.title = termina de configurar en tu navegador

journal.mac.setup.folder.holds.another.journal = esta carpeta se superpone con otro diario en esta mac. elige una carpeta distinta.

journal.mac.setup.go.back.to.the.journal.in = volver al diario en { $path }

journal.mac.setup.handoff.competing.setup = la app de { $product } ya está terminando otra configuración inicial. termínala ahí, o abre este diario aquí.

journal.mac.setup.handoff.held.pairing = la app de { $product } ya está conectada a un diario. abre la app de { $product } para cambiar esa conexión, o abre este diario aquí.

journal.mac.setup.handoff.not.delivered = no se pudo continuar la configuración inicial en la app de { $product }.

journal.mac.setup.handoff.pending = la configuración inicial no ha continuado en la app de { $product }. ábrela para continuar, o vuelve a intentarlo aquí.

journal.mac.setup.journal.folder = carpeta del diario

journal.mac.setup.location.field = ubicación

journal.mac.setup.login.item.change.failed = no se pudo cambiar el arranque al iniciar sesión.

journal.mac.setup.login.item.unavailable = el arranque al iniciar sesión no está disponible desde esta ubicación de la app.

journal.mac.setup.name.location.title = vamos a crear tu diario en esta mac

journal.mac.setup.on.this.mac = en esta mac

journal.mac.setup.on.this.mac.description = las solicitudes del modelo se quedan en esta mac. continuar inicia la descarga.

journal.mac.setup.open.your.journal = abrir tu diario

journal.mac.setup.reopen.in.browser.button = abrir la página de configuración inicial

journal.mac.setup.restore.from.backup = restaurar desde una copia de seguridad

journal.mac.setup.runner.solstone.journal.setup.failed = { $setup_command } falló

journal.mac.setup.set.up.solstone = configurar { $product }

journal.mac.setup.setup.progress.not.saved = no se pudo guardar el avance de la configuración inicial.

journal.mac.setup.setup.subtitle = esto puede tardar un minuto.

journal.mac.setup.setup.title = configurando tu diario

journal.mac.setup.start.automatically.help = se inicia automáticamente después de que inicias sesión en esta mac.

journal.mac.setup.start.setup = iniciar la configuración inicial

journal.mac.setup.step.progress = paso { $step } de 7

journal.mac.setup.the.solstone.app = la app de { $product }

journal.mac.setup.the.solstone.app.description = se ejecuta en esta mac. tu micrófono, tu pantalla y el sonido de la computadora permanecen desactivados hasta que los actives.

journal.mac.setup.thinking = razonamiento

journal.mac.setup.thinking.description = ayuda a tu diario a darle sentido a lo que guarda. eliges dónde durante la configuración inicial, o más tarde.

journal.mac.setup.thinking.failed = no se pudo terminar este paso. vuelve a intentarlo.

journal.mac.setup.try.again = reintentar

journal.mac.setup.what.this.sets.up = qué configura esto

journal.mac.setup.what.this.sets.up.journal = vive en esta mac y guarda todo lo que entra en él.

journal.mac.setup.where.does.thinking.happen = ¿dónde ocurre el razonamiento?

journal.mac.setup.where.does.thinking.happen.description = tu diario se queda en esta mac elijas lo que elijas. esto solo decide dónde ocurre el razonamiento.

journal.mac.setup.where.your.journal.lives = dónde vive tu diario

journal.mac.setup.where.your.journal.lives.description = todo lo que tu diario guarda se queda en esta carpeta, en esta mac.

journal.mac.setup.your.journal = tu diario

journal.mac.updates.deferred = la actualización de tu diario continuará cuando tu diario esté listo.

journal.mac.updates.ready.to.install = la app del diario { $version } está descargada y lista para instalar.

journal.mac.window.model.backup = copia de seguridad

journal.mac.window.model.blocked = bloqueado

journal.mac.window.model.devices = dispositivos

journal.mac.window.model.healthy = en buen estado

journal.mac.window.model.home = inicio

journal.mac.window.model.journal = journal

journal.mac.window.model.location = ubicación

journal.mac.window.model.not.set = sin definir

journal.mac.window.model.run.state = estado de ejecución

journal.mac.window.model.running = en ejecución

journal.mac.window.model.starting = iniciando…

journal.mac.window.model.startup = arranque

journal.mac.window.model.stopped = detenido

journal.mac.window.model.unknown = desconocido

journal.mac.window.model.updates = actualizaciones
