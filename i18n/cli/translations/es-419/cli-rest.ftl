journal.cli.backfill.committed = CONFIRMADO

journal.cli.backfill.day.missing = No se encontró el día { $day } en el diario

journal.cli.backfill.days = No se pudieron listar los días de la crónica: { $error }

journal.cli.backfill.flags.conflict = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: error: argumento { $flag }: no se permite con el argumento { $previous }

journal.cli.backfill.flags.missing = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: error: argumento { $day_flag }: se esperaba un argumento

journal.cli.backfill.flags.unknown = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: error: argumentos no reconocidos: { $argument }

journal.cli.backfill.help = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $line_break }Rellena registros de procesamiento vacíos en las salidas nativas atascadas de solo encabezado de{ $line_break }{ $handlers }{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_5 }muestra este mensaje de ayuda y sal{ $line_break }{ $indent_2 }{ $day_option }{ $gap_6 }Día en formato { $date_format }; de forma predeterminada, todos los días{ $line_break }{ $indent_2 }{ $commit_option }{ $gap_7 }Reescribe los encabezados { $jsonl_format } elegibles con registros de procesamiento vacíos{ $line_break }{ $indent_2 }{ $dry_run_option }{ $gap_6 }Previsualiza las salidas elegibles sin escribir cambios{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_2 }Activa la salida detallada{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_4 }Activa el registro de depuración

journal.cli.backfill.identity.ambiguous = el directorio de flujo con nombre "{ $default_stream }" no se puede expresar como identidad de registro: { $path }

journal.cli.backfill.identity.duplicate = varios segmentos comparten el flujo { $stream } con la clave { $key }

journal.cli.backfill.identity.encoding = la ruta del segmento no se puede representar en { $encoding }: { $path }

journal.cli.backfill.invalid.day = se esperaba un día en formato { $date_format }

journal.cli.backfill.marker = Se estampó { $path }, pero no se pudo marcar el día { $day } como actualizado: { $error }

journal.cli.backfill.preview = SIMULACIÓN (no se escribió ningún cambio)

journal.cli.backfill.segment.directory = No se pudo listar el directorio de segmentos { $path }: { $error }

journal.cli.backfill.segments = No se pudieron listar los segmentos del día { $day }: { $error }

journal.cli.backfill.stamp.changed = No se pudo estampar { $path }: el archivo cambió después de la clasificación

journal.cli.backfill.stamp.read = No se pudo estampar { $path }: no se pudo volver a leer el archivo: { $error }

journal.cli.backfill.stamp.write = No se pudo estampar { $path }: no se pudo escribir: { $error }

journal.cli.backfill.stream.directory = No se pudo listar el directorio de flujos { $path }: no es un directorio

journal.cli.backfill.stream.encoding = el flujo del segmento no se puede representar en { $encoding }: { $path }

journal.cli.brain.arguments.invalid = { $command }: error: argumentos no válidos

journal.cli.brain.busy = Razonamiento ocupado: ya se está ejecutando una revisión

journal.cli.brain.check.config = no se pudo revisar el razonamiento: no se pudo leer la configuración del diario: { $detail }

journal.cli.brain.check.failure = no se pudo revisar el razonamiento: { $detail }

journal.cli.brain.check.fingerprint.options = la huella activa esperada y la ausencia esperada se excluyen mutuamente

journal.cli.brain.check.key.missing = el proveedor necesita una clave, y tu diario no pudo confirmar que el resultado se guardó.

journal.cli.brain.check.key.rejected = el proveedor rechazó esta clave, y tu diario no pudo confirmar que el resultado se guardó.

journal.cli.brain.check.open.directory = no se pudo abrir el descendiente de directorio plano de { $path }: { $reason }

journal.cli.brain.check.random = no se pudieron generar bytes aleatorios seguros

journal.cli.brain.heading.blocked = Razonamiento bloqueado: { $reason }

journal.cli.brain.heading.blocked.component = Razonamiento bloqueado: { $reason } ({ $component })

journal.cli.brain.heading.checking = Razonamiento revisando: { $reason }

journal.cli.brain.heading.checking.component = Razonamiento revisando: { $reason } ({ $component })

journal.cli.brain.heading.other = Razonamiento { $state }: { $reason }

journal.cli.brain.heading.other.component = Razonamiento { $state }: { $reason } ({ $component })

journal.cli.brain.heading.unhealthy = Razonamiento en mal estado: { $reason }

journal.cli.brain.heading.unhealthy.component = Razonamiento en mal estado: { $reason } ({ $component })

journal.cli.brain.heading.unknown = Razonamiento desconocido: { $reason }

journal.cli.brain.heading.unknown.component = Razonamiento desconocido: { $reason } ({ $component })

journal.cli.brain.help.refresh = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }muestra este mensaje de ayuda y sal{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Emite { $json_format } en lugar de la salida simple{ $line_break }{ $syntax_continuation_7 }{ $line_break }{ $syntax_continuation_8 }{ $line_break }{ $syntax_continuation_9 }

journal.cli.brain.help.renew = uso: { $syntax }{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }muestra este mensaje de ayuda y sal

journal.cli.brain.help.root = uso: { $syntax }{ $line_break }{ $line_break }CLI de estado del razonamiento activo y de actualización acotada.{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $syntax_continuation_5 }{ $line_break }    { $status }Muestra el estado del razonamiento activo{ $line_break }    { $refresh }Ejecuta una revisión acotada del razonamiento activo{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_10 }muestra este mensaje de ayuda y sal

journal.cli.brain.help.status = uso: { $syntax }{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }muestra este mensaje de ayuda y sal{ $line_break }{ $indent_2 }{ $json_option }{ $gap_6 }Emite { $json_format } en lugar de la salida simple

journal.cli.brain.hold.capacity = la capacidad de reserva de la credencial no está disponible

journal.cli.brain.hold.invalid.provider = proveedor de reserva de credencial no admitido

journal.cli.brain.hold.root.changed = cambió la raíz de diario admitida

journal.cli.brain.hold.superseded = la validación de la credencial fue sustituida

journal.cli.brain.hold.unavailable = el estado de la reserva de la credencial no está disponible

journal.cli.brain.ready.byo = Razonamiento listo: tu propio modelo

journal.cli.brain.ready.byo.checked = Razonamiento listo: tu propio modelo, se revisó hace { $age }

journal.cli.brain.ready.byo.model = Razonamiento listo: tu propio modelo { $model }

journal.cli.brain.ready.byo.model.checked = Razonamiento listo: tu propio modelo { $model }, se revisó hace { $age }

journal.cli.brain.ready.local = Razonamiento listo: razonamiento local

journal.cli.brain.ready.local.checked = Razonamiento listo: razonamiento local, se revisó hace { $age }

journal.cli.brain.ready.local.model = Razonamiento listo: razonamiento local { $model }

journal.cli.brain.ready.local.model.checked = Razonamiento listo: razonamiento local { $model }, se revisó hace { $age }

journal.cli.brain.ready.other = Razonamiento listo: { $lane }

journal.cli.brain.ready.other.checked = Razonamiento listo: { $lane }, se revisó hace { $age }

journal.cli.brain.ready.other.model = Razonamiento listo: { $lane } { $model }

journal.cli.brain.ready.other.model.checked = Razonamiento listo: { $lane } { $model }, se revisó hace { $age }

journal.cli.brain.ready.spp = Razonamiento listo: procesamiento confidencial

journal.cli.brain.ready.spp.checked = Razonamiento listo: procesamiento confidencial, se revisó hace { $age }

journal.cli.brain.ready.spp.model = Razonamiento listo: procesamiento confidencial { $model }

journal.cli.brain.ready.spp.model.checked = Razonamiento listo: procesamiento confidencial { $model }, se revisó hace { $age }

journal.cli.brain.ready.unknown = Razonamiento listo: desconocido

journal.cli.brain.ready.unknown.checked = Razonamiento listo: desconocido, se revisó hace { $age }

journal.cli.brain.ready.unknown.model = Razonamiento listo: desconocido { $model }

journal.cli.brain.ready.unknown.model.checked = Razonamiento listo: desconocido { $model }, se revisó hace { $age }

journal.cli.brain.settlement.fingerprint.changed = la clave de huella protegida cambió después de la validación

journal.cli.brain.settlement.publication.changed = la validación de la credencial cambió después de la publicación

journal.cli.brain.settlement.root.changed = cambió la ruta de diario admitida

journal.cli.brain.settlement.selection.changed = la selección de credencial cambió durante la actualización

journal.cli.brain.settlement.unconfirmed = no se confirmó la publicación de la validación de la credencial

journal.cli.brain.settlement.validation.changed = la validación de la credencial cambió después de la actualización

journal.cli.brain.usage = uso: { $syntax }

journal.cli.brain.validation.checking.aggregate.requires.matching.checking.marker = el agregado de revisión requiere un marcador de revisión que coincida

journal.cli.brain.validation.checking.reason.is.not.valid.component.evidence = el motivo de revisión no es evidencia válida del componente

journal.cli.brain.validation.component.status.does.not.match.reason.aggregate = el estado del componente no coincide con el agregado del motivo

journal.cli.brain.validation.detail = { $field }: { $reason }

journal.cli.brain.validation.diagnostic.enum.value.not.allowed = valor de enumeración de diagnóstico no permitido

journal.cli.brain.validation.diagnostic.key.not.allowed = clave de diagnóstico no permitida

journal.cli.brain.validation.diagnostic.value.must.be.an.enum.string = el valor de diagnóstico debe ser una cadena de enumeración

journal.cli.brain.validation.does.not.match.aggregate.state = no coincide con { $token_aggregate_state }

journal.cli.brain.validation.expected.sha256.hex.string = se esperaba una cadena hexadecimal { $token_sha_256 }

journal.cli.brain.validation.is.required.when.status.is.ok = se requiere cuando el estado es ok

journal.cli.brain.validation.missing.lane.applicable.evidence.without.higher.priority.cause = falta evidencia aplicable al carril sin una causa de mayor prioridad

journal.cli.brain.validation.must.be.a.non.negative.integer = debe ser un entero no negativo

journal.cli.brain.validation.must.be.a.sha256.hex.digest = debe ser un resumen hexadecimal { $token_sha_256 }

journal.cli.brain.validation.must.be.a.string = debe ser una cadena

journal.cli.brain.validation.must.be.a.string.or.null = debe ser una cadena o null

journal.cli.brain.validation.must.be.an.integer = debe ser un entero

journal.cli.brain.validation.must.be.an.iso8601.timestamp = debe ser una marca de tiempo { $token_iso_8601 }

journal.cli.brain.validation.must.be.an.object = debe ser un objeto

journal.cli.brain.validation.must.be.evidence.recordable = debe ser registrable como evidencia

journal.cli.brain.validation.must.be.non.empty = debe tener contenido

journal.cli.brain.validation.must.be.null.when.aggregate.state.is.ready = debe ser null cuando { $token_aggregate_state } está listo

journal.cli.brain.validation.must.be.provider.key.invalid = debe ser { $token_provider_key_invalid }

journal.cli.brain.validation.must.be.set.when.aggregate.state.is.not.ready = debe tener un valor cuando { $token_aggregate_state } no está listo

journal.cli.brain.validation.non.ok.evidence.requires.reason = la evidencia que no es ok requiere un motivo

journal.cli.brain.validation.not.attempted.is.only.valid.for.generate = { $token_not_attempted } solo es válido para generate

journal.cli.brain.validation.not.attempted.reason.must.repeat.causal.prerequisite.reason = el motivo de { $token_not_attempted } debe repetir el motivo del requisito previo causal

journal.cli.brain.validation.not.attempted.requires.a.non.ok.prerequisite = { $token_not_attempted } requiere un requisito previo que no es ok

journal.cli.brain.validation.ok.evidence.requires.null.reason = la evidencia ok requiere un motivo null

journal.cli.brain.validation.prerequisite.renewal.requires.ok.or.declared.failure = la renovación del requisito previo requiere ok o un fallo declarado

journal.cli.brain.validation.reason.not.allowed.for.evidence.component = motivo no permitido para el componente de evidencia

journal.cli.brain.validation.record.aggregate.reason.does.not.match.evidence = el agregado/motivo del registro no coincide con la evidencia

journal.cli.brain.validation.unknown.field = campo desconocido

journal.cli.brain.validation.unknown.top.level.field = campo de nivel superior desconocido

journal.cli.brain.validation.unknown.value = valor desconocido

journal.cli.brain.validation.unsupported.binding.version = versión de enlace no admitida

journal.cli.brain.validation.unsupported.schema.version = versión de esquema no admitida

journal.cli.brain.writer.active.brain.fingerprint.is.unavailable = la huella del razonamiento activo no está disponible

journal.cli.brain.writer.brain.abandon.reason.is.not.recordable.evidence = el motivo de abandono del razonamiento no es evidencia registrable

journal.cli.brain.writer.brain.fingerprint.changed = cambió la huella del razonamiento

journal.cli.brain.writer.brain.fingerprint.key.is.unavailable = la clave de la huella del razonamiento no está disponible

journal.cli.brain.writer.brain.lane.changed = cambió el carril del razonamiento

journal.cli.brain.writer.brain.prerequisite.evidence.is.unsafe = la evidencia de requisito previo del razonamiento no es segura

journal.cli.brain.writer.brain.record.changed.during.inspection = Cambió el registro de razonamiento durante la inspección

journal.cli.brain.writer.brain.refresh.checking.marker.is.absent = falta el marcador de revisión de la actualización del razonamiento

journal.cli.brain.writer.brain.refresh.permit.expired = el permiso de actualización del razonamiento está vencido

journal.cli.brain.writer.brain.refresh.record.revision.changed = cambió la revisión del registro de la actualización del razonamiento

journal.cli.brain.writer.brain.refresh.revision.changed = cambió la revisión de la actualización del razonamiento

journal.cli.brain.writer.brain.refresh.run.id.changed = cambió el id de ejecución de actualización del razonamiento

journal.cli.brain.writer.brain.runtime.failure.marker.changed = cambió el marcador de fallo del tiempo en ejecución del razonamiento

journal.cli.brain.writer.configuration.rejection.changed.after.publication = cambió el rechazo de la configuración después de la publicación

journal.cli.brain.writer.credential.authority.changed.during.inspection = cambió la autoridad de la credencial durante la inspección

journal.cli.brain.writer.credential.authority.changed.during.observation = cambió la autoridad de la credencial durante la observación

journal.cli.brain.writer.credential.authority.is.not.an.object = la autoridad de la credencial no es un objeto

journal.cli.brain.writer.credential.authority.is.unreadable = la autoridad de la credencial no se puede leer

journal.cli.brain.writer.credential.binding.authority.could.not.be.read.error = no se pudo leer la autoridad de enlace de la credencial: { $error }

journal.cli.brain.writer.credential.hold.authority.changed = cambió la autoridad de reserva de la credencial

journal.cli.brain.writer.credential.hold.authority.is.not.an.object = la autoridad de reserva de la credencial no es un objeto

journal.cli.brain.writer.credential.hold.authority.is.unreadable.error = la autoridad de reserva de la credencial no se puede leer: { $error }

journal.cli.brain.writer.credential.observation.authority.is.not.an.object = la autoridad de observación de la credencial no es un objeto

journal.cli.brain.writer.credential.observation.authority.is.unreadable = la autoridad de observación de la credencial no se puede leer

journal.cli.brain.writer.credential.observation.changed.during.refresh = cambió la observación de la credencial durante la actualización

journal.cli.brain.writer.credential.observation.changed.during.work = cambió la observación de la credencial durante el trabajo

journal.cli.brain.writer.credential.observation.identity.is.invalid = la identidad de la observación de la credencial no es válida

journal.cli.brain.writer.credential.observation.lacks.original.binding = la observación de la credencial no tiene el enlace original

journal.cli.brain.writer.credential.observations.are.not.an.object = las observaciones de la credencial no son un objeto

journal.cli.brain.writer.credential.process.state.changed.during.inspection = cambió el estado del proceso de la credencial durante la inspección

journal.cli.brain.writer.credential.publication.reservation.is.absent = la reserva de publicación de la credencial está ausente

journal.cli.brain.writer.credential.rejection.changed.after.publication = cambió el rechazo de la credencial después de la publicación

journal.cli.brain.writer.credential.rejection.changed.during.refresh = cambió el rechazo de la credencial durante la actualización

journal.cli.brain.writer.credential.rejection.changed.during.validation = cambió el rechazo de la credencial durante la validación

journal.cli.brain.writer.field.must.be.an.object = { $field } debe ser un objeto

journal.cli.brain.writer.protected.fingerprint.key.changed.after.rejection = cambió la clave de huella protegida después del rechazo

journal.cli.brain.writer.protected.fingerprint.key.changed.during.admission = cambió la clave de huella protegida durante la admisión

journal.cli.brain.writer.protected.fingerprint.key.changed.during.inspection = cambió la clave de huella protegida durante la inspección

journal.cli.brain.writer.protected.fingerprint.key.changed.during.validation = cambió la clave de huella protegida durante la validación

journal.cli.brain.writer.protected.fingerprint.key.is.absent.while.a.credential.binding.exists = la clave de huella protegida está ausente mientras existe un enlace de credencial

journal.cli.brain.writer.protected.fingerprint.key.is.absent.while.credential.authority.exists = la clave de huella protegida está ausente mientras existe la autoridad de la credencial

journal.cli.brain.writer.protected.fingerprint.key.is.unavailable = la clave de huella protegida no está disponible

journal.cli.brain.writer.reason.is.not.recordable.evidence = el motivo no es evidencia registrable

journal.cli.brain.writer.rejected.refresh.receipt.does.not.match.its.probe = el recibo de actualización rechazado no coincide con su sondeo

journal.cli.brain.writer.rejection.receipt.root.provider.changed = cambió la raíz/proveedor del recibo de rechazo

journal.cli.brain.writer.settled.credential.observation.changed.during.work = cambió la observación de credencial asentada durante el trabajo

journal.cli.brain.writer.validation.result.does.not.match.its.credential = el resultado de la validación no coincide con su credencial

journal.cli.brain.writer.validation.result.is.not.an.object = el resultado de la validación no es un objeto

journal.cli.depict.admission = no se pudo admitir el proceso del diario: { $detail }

journal.cli.depict.admission.binding.mismatch = discrepancia de enlace de la instalación de lanzamiento

journal.cli.depict.admission.birth.invalid = el nacimiento del proceso de { $linux } tiene un { $boot } con forma incorrecta

journal.cli.depict.admission.birth.position = el nacimiento del proceso de { $linux } tiene un { $boot } con forma incorrecta en la línea { $line }, columna { $column }

journal.cli.depict.admission.byte.count = la tubería de lanzamiento devolvió un recuento de bytes no válido

journal.cli.depict.admission.cleanup.pending = la limpieza de { $io } del lanzamiento anterior sigue incompleta

journal.cli.depict.admission.commit.mismatch = discrepancia de confirmación en la admisión de lanzamiento

journal.cli.depict.admission.deadline.elapsed = transcurrió el plazo de lanzamiento

journal.cli.depict.admission.deadline.invalid = plazo de lanzamiento de { $windows } vencido o no válido

journal.cli.depict.admission.descriptor.encoding = descriptor de lanzamiento no { $unicode } de { $windows }

journal.cli.depict.admission.descriptor.size = descriptor de lanzamiento de { $windows } demasiado grande

journal.cli.depict.admission.endpoint = endpoint de lanzamiento no válido

journal.cli.depict.admission.entry.hosted = tarea instalada en la entrada alojada

journal.cli.depict.admission.entry.mismatch = discrepancia de variante de lanzamiento o de acción instalada

journal.cli.depict.admission.frame.size = tamaño de marco de lanzamiento no válido

journal.cli.depict.admission.grant.access = el acceso o el tipo del identificador de lanzamiento no coincide con su concesión

journal.cli.depict.admission.grant.disk = la concesión de lanzamiento no es un archivo de disco

journal.cli.depict.admission.grant.duplicate = identificador de lanzamiento o tipo de concesión duplicado

journal.cli.depict.admission.grant.ordinary = la concesión de lanzamiento no es un archivo de disco ordinario

journal.cli.depict.admission.guard.duplicate = guarda de instalación: guarda de identidad duplicada { $field }

journal.cli.depict.admission.guard.encoding = guarda de instalación que no es { $unicode }

journal.cli.depict.admission.guard.invalid = guarda de instalación: guarda de identidad no válida { $field }

journal.cli.depict.admission.guard.mismatch = la tupla de guarda de lanzamiento difiere de la guarda heredada

journal.cli.depict.admission.guard.missing = guarda de instalación: los campos de la guarda de identidad están incompletos

journal.cli.depict.admission.handle.invalid = identificador transferido no válido

journal.cli.depict.admission.identity.io.local.folder = resolver la carpeta conocida { $local_data }: { $error }

journal.cli.depict.admission.identity.io.marker.clone = clonar el marcador de adopción: { $error }

journal.cli.depict.admission.identity.io.marker.read = leer el marcador de adopción: { $error }

journal.cli.depict.admission.identity.io.marker.stat = consultar el marcador de adopción: { $error }

journal.cli.depict.admission.identity.io.namespace.read = leer el directorio del espacio de nombres: { $error }

journal.cli.depict.admission.identity.io.owner.open = abrir el directorio del propietario: { $error }

journal.cli.depict.admission.identity.io.record.read = leer el registro de identidad: { $error }

journal.cli.depict.admission.identity.io.record.stat = consultar el registro de identidad: { $error }

journal.cli.depict.admission.identity.io.root = canonizar la raíz: { $error }

journal.cli.depict.admission.identity.io.storage.attrs = consultar los atributos de almacenamiento: { $error }

journal.cli.depict.admission.identity.io.storage.file = abrir el archivo de almacenamiento: { $error }

journal.cli.depict.admission.identity.io.storage.identity = consultar la identidad de almacenamiento: { $error }

journal.cli.depict.admission.identity.io.storage.lock = bloquear el almacenamiento de identidad: { $error }

journal.cli.depict.admission.identity.io.storage.open = abrir el directorio de almacenamiento: { $error }

journal.cli.depict.admission.identity.local.encoding = la carpeta conocida { $local_data } no tiene una codificación { $utf } válida

journal.cli.depict.admission.identity.marker.content = el marcador de adopción tiene contenido no válido

journal.cli.depict.admission.identity.marker.missing = el registro confirmado no tiene marcador de adopción

journal.cli.depict.admission.identity.marker.orphan = marcador de adopción huérfano

journal.cli.depict.admission.identity.marker.size = el marcador de adopción tiene un tamaño no válido

journal.cli.depict.admission.identity.namespace.child = elemento secundario de espacio de nombres desconocido

journal.cli.depict.admission.identity.owner.poisoned = el coordinador del propietario está envenenado

journal.cli.depict.admission.identity.path.device = no se aceptan las rutas verbatim ni de dispositivo de { $windows }

journal.cli.depict.admission.identity.path.encoding = la ruta de { $windows } no tiene una codificación { $utf } válida

journal.cli.depict.admission.identity.path.escape = la ruta de { $windows } se sale de su raíz absoluta

journal.cli.depict.admission.identity.path.nul = la ruta de { $windows } contiene { $nul } en su interior

journal.cli.depict.admission.identity.path.relative = la ruta de { $windows } debe ser totalmente calificada

journal.cli.depict.admission.identity.path.server = la ruta de { $windows } { $unc } no tiene servidor

journal.cli.depict.admission.identity.path.share = la ruta de { $windows } { $unc } no tiene recurso compartido

journal.cli.depict.admission.identity.path.size = la ruta supera { $limit } bytes

journal.cli.depict.admission.identity.prepared = el registro de identidad de la instalación está preparado

journal.cli.depict.admission.identity.record.missing = falta el registro del espacio de nombres

journal.cli.depict.admission.identity.record.size = el registro de identidad supera los límites

journal.cli.depict.admission.identity.root.mismatch = el registro no enlaza la raíz solicitada

journal.cli.depict.admission.identity.storage.directory = la ruta de almacenamiento no es un directorio

journal.cli.depict.admission.identity.storage.file = la ruta de almacenamiento no es un archivo regular

journal.cli.depict.admission.identity.storage.reparse = la ruta de almacenamiento es un punto de reanálisis

journal.cli.depict.admission.identity.tombstoned = el registro de identidad de la instalación está marcado como eliminado

journal.cli.depict.admission.nonce = nonce de endpoint de lanzamiento no válido

journal.cli.depict.admission.obsolete = marcador de lanzamiento de { $windows } obsoleto

journal.cli.depict.admission.offer.mismatch = discrepancia de procedencia de la oferta de lanzamiento

journal.cli.depict.admission.parent.mismatch = el servidor de lanzamiento no es el padre directo exacto

journal.cli.depict.admission.parent.provenance = procedencia de lanzamiento del padre no válida

journal.cli.depict.admission.peer.unverified = el par de la tubería de lanzamiento no está verificablemente activo

journal.cli.depict.admission.process.creation = el proceso actual de { $windows } no tiene disponible su { $filetime } de creación

journal.cli.depict.admission.process.signaled = el proceso actual de { $windows } está señalado

journal.cli.depict.admission.provenance = procedencia de lanzamiento no válida

journal.cli.depict.admission.record.ascii = el registro de identidad no es { $ascii }

journal.cli.depict.admission.record.checksum = discrepancia de suma de verificación del registro de identidad

journal.cli.depict.admission.record.field = registro de identidad no válido { $field }

journal.cli.depict.admission.record.lines = el registro de identidad no es exactamente ocho líneas { $linefeed }

journal.cli.depict.admission.record.size = el registro de identidad supera { $size }

journal.cli.depict.admission.root.missing = el ejecutable de lanzamiento no tiene raíz de instalación

journal.cli.depict.admission.server.mismatch = discrepancia de identidad del servidor de la tubería de lanzamiento

journal.cli.depict.admission.snapshot.absent = el proceso actual está ausente de la instantánea del padre

journal.cli.depict.admission.task.provenance = procedencia de la tarea instalada no válida

journal.cli.depict.admission.wait.complete = no se pudo esperar el { $io } del lanzamiento: { $error }; se completó la cancelación de { $io }

journal.cli.depict.admission.wait.deadline.complete = transcurrió el plazo de lanzamiento; se completó la cancelación de { $io }

journal.cli.depict.admission.wait.deadline.incomplete = transcurrió el plazo de lanzamiento; la limpieza de { $io } quedó incompleta y se conservó

journal.cli.depict.admission.wait.incomplete = no se pudo completar la espera de { $io } del lanzamiento: { $error }; la limpieza de { $io } quedó incompleta y se conservó

journal.cli.depict.detection.unavailable = La detección de objetos no está disponible. Ejecuta `{ $command }` para revisar o reparar los recursos de { $engine }.

journal.cli.depict.detection.unavailable.windows = la detección de objetos no está disponible. repara o reinstala tu diario.

journal.cli.depict.help = uso: { $syntax }

journal.cli.depict.metadata.invalid = depict nativo: { $variable } { $format } no válido

journal.cli.describe.cancelled = describe cancelado

journal.cli.describe.decode.failed = no se pudo decodificar el video: { $path }

journal.cli.describe.deferred = describe diferido

journal.cli.describe.deferred.reason = describe diferido: { $reason }

journal.cli.describe.faces.skipped = se omitió el enriquecimiento de rostros: { $kind }

journal.cli.describe.file.encoding = la ruta del video debe tener un nombre de archivo { $encoding }

journal.cli.describe.output.encode = no se pudo codificar la salida: { $error }

journal.cli.describe.output.write = no se pudo escribir la salida: { $error }

journal.cli.describe.path.config = no se pudo leer la ruta del diario: { $reason }

journal.cli.describe.path.home = no se pudo resolver el directorio de inicio: { $reason }

journal.cli.describe.pipeline.flat.bound = el nombre del directorio enlazado existe y no es un directorio

journal.cli.describe.pipeline.flat.create.flat.directory.child = no se pudo crear el hijo del directorio plano para { $path }: { $detail }

journal.cli.describe.pipeline.flat.normal = el nombre debe ser un solo componente normal

journal.cli.describe.pipeline.flat.read.flat.directory.identity = no se pudo leer la identidad del directorio plano para { $path }: { $detail }

journal.cli.describe.pipeline.flat.relative = la ruta debe ser una secuencia no vacía de componentes normales portables

journal.cli.describe.pipeline.flat.revalidate.flat.directory = no se pudo revalidar el directorio plano para { $path }: { $detail }

journal.cli.describe.pipeline.flat.stat.flat.directory.descendant = no se pudo hacer stat del descendiente del directorio plano para { $path }: { $detail }

journal.cli.describe.pipeline.flat.stat.opened.flat.directory.descendant = no se pudo hacer stat del descendiente abierto del directorio plano para { $path }: { $detail }

journal.cli.describe.pipeline.flat.utf8 = el nombre no es texto { $encoding } válido

journal.cli.describe.pipeline.frames.failed = fallaron todas las solicitudes de fotogramas de describe

journal.cli.describe.pipeline.install.observe.installed.destination.after.move = { $path }: observar el destino instalado después del movimiento: { $detail }

journal.cli.describe.pipeline.install.reconcile.install.after.failed.move = { $path }: conciliar la instalación después del movimiento fallido: { $detail }

journal.cli.describe.pipeline.install.revalidate.install.paths.after.move = { $path }: revalidar las rutas de instalación después del movimiento: { $detail }

journal.cli.describe.pipeline.jobs = { $flag } debe ser positivo

journal.cli.describe.pipeline.merge.frame = falta el fotograma de combinación

journal.cli.describe.pipeline.metadata.object = { $segment_meta } debe ser un objeto

journal.cli.describe.pipeline.ocr.coverage = la cobertura de { $ocr } de pantalla está incompleta; los diagnósticos privados de fotogramas registran la brecha

journal.cli.describe.pipeline.ocr.publication = falló la publicación de { $ocr } de pantalla: { $kind }

journal.cli.describe.pipeline.ocr.retire = no se pudo retirar el agregado desactualizado de { $ocr } de pantalla: { $kind }

journal.cli.describe.pipeline.parent = el video no tiene padre

journal.cli.describe.pipeline.progress = la ventana de fotogramas de describe no avanzó

journal.cli.describe.pipeline.publish.anchor.is.not.a.mount.manager.volume.guid.path = el ancla no es una ruta de volumen { $guid } de { $manager }

journal.cli.describe.pipeline.publish.changed = la publicación preparada cambió

journal.cli.describe.pipeline.publish.component = la hoja de la publicación no es un componente simple

journal.cli.describe.pipeline.publish.current.directory.is.not.valid.utf = el directorio actual no es { $encoding } válido

journal.cli.describe.pipeline.publish.destination = el destino de la publicación no es un archivo simple

journal.cli.describe.pipeline.publish.destination.already.identifies.the.source.file = el destino ya identifica el archivo de origen

journal.cli.describe.pipeline.publish.destination.changed.during.publication = el destino cambió durante la publicación

journal.cli.describe.pipeline.publish.destination.has.no.normal.name = el destino no tiene un nombre normal

journal.cli.describe.pipeline.publish.destination.is.a.reparse.point = el destino es un punto de reanálisis

journal.cli.describe.pipeline.publish.destination.is.not.a.regular.file = el destino no es un archivo normal

journal.cli.describe.pipeline.publish.identity = la identidad de la publicación preparada cambió

journal.cli.describe.pipeline.publish.identity.value.out.of.range = el valor de identidad está fuera de rango

journal.cli.describe.pipeline.publish.install.publication.exhausted = se agotó la publicación de la instalación

journal.cli.describe.pipeline.publish.leaf = falta la hoja de la publicación

journal.cli.describe.pipeline.publish.mode.exceeds.o = el modo supera { $octal }

journal.cli.describe.pipeline.publish.move.spelling.contains.an.interior.nul = la grafía del movimiento contiene un { $nul } interior

journal.cli.describe.pipeline.publish.parent = falta el padre de la publicación

journal.cli.describe.pipeline.publish.path = { $path }: { $detail }

journal.cli.describe.pipeline.publish.publication.path.is.not.valid.utf = la ruta de publicación no es { $encoding } válido

journal.cli.describe.pipeline.publish.published.destination.identity.does.not.match.the.live.source = la identidad del destino publicado no coincide con el origen en vivo

journal.cli.describe.pipeline.publish.published.destination.is.not.a.regular.file = el destino publicado no es un archivo normal

journal.cli.describe.pipeline.publish.published.destination.name.is.absent = el nombre del destino publicado está ausente

journal.cli.describe.pipeline.publish.source.and.destination.are.on.different.volumes = el origen y el destino están en volúmenes distintos

journal.cli.describe.pipeline.publish.source.and.destination.name.the.same.file = el origen y el destino nombran el mismo archivo

journal.cli.describe.pipeline.publish.source.has.no.normal.name = el origen no tiene un nombre normal

journal.cli.describe.pipeline.publish.source.identity.changed.before.publication = la identidad del origen cambió antes de la publicación

journal.cli.describe.pipeline.publish.source.identity.is.negative = la identidad del origen es negativa

journal.cli.describe.pipeline.publish.source.name.changed.before.publication = el nombre del origen cambió antes de la publicación

journal.cli.describe.pipeline.publish.source.name.is.absent.before.publication = el nombre del origen está ausente antes de la publicación

journal.cli.describe.pipeline.publish.sync = no se pudo sincronizar el padre de la publicación de describe: { $error }

journal.cli.describe.pipeline.publish.uncertain = { $path }: { $operation }: { $detail }

journal.cli.describe.pipeline.reappear = el fotograma { $frame } de describe no reapareció con la misma marca de tiempo

journal.cli.describe.pipeline.record.parent.changed = el padre del texto de pantalla cambió

journal.cli.describe.pipeline.record.parent.plain = el padre del texto de pantalla no es un directorio simple

journal.cli.describe.pipeline.reopen = no se pudo reabrir el video desde describe para materializar las cargas útiles de los fotogramas

journal.cli.describe.pipeline.resize = no se pudo cambiar el tamaño del fotograma de categorización

journal.cli.describe.pipeline.response.id = la respuesta de la sesión omitió el id

journal.cli.describe.pipeline.response.unmatched = respuesta no correlacionada

journal.cli.describe.pipeline.retry.binding = el enlace del fotograma de reintento de { $ocr } cambió

journal.cli.describe.pipeline.retry.evidence.missing = falta la evidencia del reintento

journal.cli.describe.pipeline.retry.handler = el manejador nativo de fallos cambió

journal.cli.describe.pipeline.retry.id = la identidad del reintento nativo no es { $encoding }

journal.cli.describe.pipeline.retry.incomplete = el reintento de { $ocr } no llegó al último fotograma calificado

journal.cli.describe.pipeline.retry.input.changed = la entrada del reintento cambió durante la lectura

journal.cli.describe.pipeline.retry.input.regular = la entrada del reintento no es un archivo normal

journal.cli.describe.pipeline.retry.io.allocate = no se pudo asignar la etapa

journal.cli.describe.pipeline.retry.io.dest.namespace = el espacio de nombres del destino cambió antes de la publicación

journal.cli.describe.pipeline.retry.io.file.kind = la entrada no es un archivo regular

journal.cli.describe.pipeline.retry.io.file.reparse = la entrada es un punto de reanálisis

journal.cli.describe.pipeline.retry.io.fsync = el fsync del directorio padre se degradó para { $path }: { $error }

journal.cli.describe.pipeline.retry.io.parent.identity = la identidad del padre enlazado cambió antes de la publicación

journal.cli.describe.pipeline.retry.io.parent.kind = el padre no es un directorio real

journal.cli.describe.pipeline.retry.io.parent.rename = el nombre de ruta del padre cambió antes de la publicación

journal.cli.describe.pipeline.retry.io.parent.reparse = el padre es un punto de reanálisis

journal.cli.describe.pipeline.retry.io.prepare = la identidad de la etapa cambió durante la preparación

journal.cli.describe.pipeline.retry.io.stage.identity = la identidad de la etapa cambió antes de la publicación

journal.cli.describe.pipeline.retry.io.stage.namespace = el espacio de nombres de la etapa cambió antes de la publicación

journal.cli.describe.pipeline.retry.io.tmp = no se pudo asignar un archivo temporal único

journal.cli.describe.pipeline.retry.journal = el diario del reintento nativo no coincide

journal.cli.describe.pipeline.retry.open = no se pudo reabrir el video para el reintento de { $ocr }

journal.cli.describe.pipeline.retry.permission.changed = el permiso del reintento nativo cambió

journal.cli.describe.pipeline.retry.permission.missing = falta el permiso del reintento nativo

journal.cli.describe.pipeline.root.canonicalize.journal.root = canonicalizar la raíz del diario: { $detail }

journal.cli.describe.pipeline.root.cloud.files.sync.root.status.could.not.be.verified = no se pudo verificar el estado de la raíz de sincronización de { $cloud_files }

journal.cli.describe.pipeline.root.filesystem.is.not.ntfs.or.refs = el sistema de archivos no es { $ntfs } ni { $refs }

journal.cli.describe.pipeline.root.filesystem.type.could.not.be.verified = no se pudo verificar el tipo de sistema de archivos

journal.cli.describe.pipeline.root.journal.root.contains.a.reparse.point = la raíz del diario contiene un punto de reanálisis

journal.cli.describe.pipeline.root.journal.root.contains.an.interior.nul = la raíz del diario contiene un { $nul } interior

journal.cli.describe.pipeline.root.journal.root.has.an.invalid.portable.name = la raíz del diario tiene un nombre portable no válido

journal.cli.describe.pipeline.root.journal.root.is.a.cloud.files.sync.root = la raíz del diario es una raíz de sincronización de { $cloud_files }

journal.cli.describe.pipeline.root.journal.root.is.a.reparse.point = la raíz del diario es un punto de reanálisis

journal.cli.describe.pipeline.root.journal.root.is.on.a.non.local.volume = la raíz del diario está en un volumen no local

journal.cli.describe.pipeline.root.journal.root.is.on.a.ram.disk = la raíz del diario está en un disco { $ram }

journal.cli.describe.pipeline.root.journal.root.is.on.removable.or.optical.media = la raíz del diario está en medios extraíbles u ópticos

journal.cli.describe.pipeline.root.journal.root.must.be.lexically.canonical = la raíz del diario debe ser léxicamente canónica

journal.cli.describe.pipeline.root.journal.root.must.use.a.fully.qualified.drive.path = la raíz del diario debe usar una ruta de unidad completa

journal.cli.describe.pipeline.root.journal.root.volume.type.is.unknown = el tipo de volumen de la raíz del diario es desconocido

journal.cli.describe.pipeline.root.open.filesystem.root = abrir la raíz del sistema de archivos: { $detail }

journal.cli.describe.pipeline.root.open.journal.directory = abrir el directorio del diario: { $detail }

journal.cli.describe.pipeline.root.open.journal.root = abrir la raíz del diario: { $detail }

journal.cli.describe.pipeline.root.open.journal.root.ancestor = abrir el ancestro de la raíz del diario: { $detail }

journal.cli.describe.pipeline.root.open.journal.root.namespace.watch = abrir la vigilancia del espacio de nombres de la raíz del diario: { $detail }

journal.cli.describe.pipeline.root.open.verified.drive.root = abrir la raíz de unidad verificada: { $detail }

journal.cli.describe.pipeline.root.open.verified.journal.root = abrir la raíz del diario verificada: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.ancestor.attributes = consultar los atributos del ancestro de la raíz del diario: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.attributes = consultar los atributos de la raíz del diario: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.drive.type = consultar el tipo de unidad de la raíz del diario: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.identity = consultar la identidad de la raíz del diario: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.namespace.watch.attributes = consultar los atributos de la vigilancia del espacio de nombres de la raíz del diario: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.namespace.watch.identity = consultar identidad de la vigilancia del espacio de nombres de la raíz del diario: { $detail }

journal.cli.describe.pipeline.root.query.retained.journal.root.attributes = consultar los atributos de la raíz del diario conservada: { $detail }

journal.cli.describe.pipeline.root.query.retained.journal.root.identity = consultar la identidad de la raíz del diario conservada: { $detail }

journal.cli.describe.pipeline.root.query.verified.drive.root.attributes = consultar los atributos de la raíz de unidad verificada: { $detail }

journal.cli.describe.pipeline.root.query.verified.drive.root.identity = consultar la identidad de la raíz de unidad verificada: { $detail }

journal.cli.describe.pipeline.root.query.verified.journal.root.attributes = consultar los atributos de la raíz del diario verificada: { $detail }

journal.cli.describe.pipeline.root.query.verified.journal.root.identity = consultar la identidad de la raíz del diario verificada: { $detail }

journal.cli.describe.pipeline.root.read.source.file.identity = leer la identidad del archivo de origen: { $detail }

journal.cli.describe.pipeline.root.restat.acquired.journal.root = volver a hacer stat de la raíz del diario adquirida: { $detail }

journal.cli.describe.pipeline.root.stat.acquired.journal.root = hacer stat de la raíz del diario adquirida: { $detail }

journal.cli.describe.pipeline.root.stat.canonical.root.component = hacer stat del componente canónico de la raíz: { $detail }

journal.cli.describe.pipeline.root.stat.opened.canonical.root.component = hacer stat del componente canónico abierto de la raíz: { $detail }

journal.cli.describe.pipeline.root.stat.opened.filesystem.root = hacer stat de la raíz abierta del sistema de archivos: { $detail }

journal.cli.describe.pipeline.row.frame = la fila de describe omitió { $frame_key }

journal.cli.describe.pipeline.source.bytes = los bytes de origen de { $ocr } cambiaron

journal.cli.describe.pipeline.source.changed = el origen de { $ocr } cambió antes de la publicación

journal.cli.describe.pipeline.source.identity = la identidad del origen de { $ocr } cambió

journal.cli.describe.pipeline.source.open = el origen de { $ocr } cambió durante la apertura

journal.cli.describe.pipeline.source.plain = el origen de { $ocr } no es un archivo simple

journal.cli.describe.pipeline.temp = no se pudo asignar el archivo temporal de describe

journal.cli.describe.pipeline.timestamp = el fotograma { $frame } de describe omitió la marca de tiempo

journal.cli.describe.pipeline.windows.cleanup = { $detail }; no se pudo quitar la etapa { $stage }: { $error }

journal.cli.describe.pipeline.windows.name.alternate.data.stream = el nombre contiene un signo de dos puntos

journal.cli.describe.pipeline.windows.name.control = el nombre contiene un carácter de control

journal.cli.describe.pipeline.windows.name.dot.component = '.' y '..' no son nombres válidos

journal.cli.describe.pipeline.windows.name.empty = el nombre está vacío

journal.cli.describe.pipeline.windows.name.forbidden.character = el nombre contiene un carácter que { $windows } no permite

journal.cli.describe.pipeline.windows.name.reserved.device = el nombre está reservado por { $windows }

journal.cli.describe.pipeline.windows.name.root.or.prefix = el nombre debe ser un solo componente relativo

journal.cli.describe.pipeline.windows.name.separator = el nombre contiene un separador de ruta

journal.cli.describe.pipeline.windows.name.too.long = el nombre es más largo que { $length_limit } bytes de { $encoding }

journal.cli.describe.pipeline.windows.name.trailing.dot.or.space = el nombre termina en un punto o un espacio

journal.cli.describe.pipeline.windows.path.already.exists = el ancestro '{ $component }' de la ruta de publicación apareció durante el modo de solo creación

journal.cli.describe.pipeline.windows.path.classify.install.source = clasificar el origen de la instalación: { $detail }

journal.cli.describe.pipeline.windows.path.component = el componente de la ruta de publicación no es portable: { $detail }

journal.cli.describe.pipeline.windows.path.create.publication.path.ancestor = crear el ancestro de la ruta de publicación: { $detail }

journal.cli.describe.pipeline.windows.path.dot.component = la ruta de publicación contiene '.' o '..'

journal.cli.describe.pipeline.windows.path.drive.relative = la ruta de publicación es relativa a la unidad

journal.cli.describe.pipeline.windows.path.empty = la ruta de publicación está vacía

journal.cli.describe.pipeline.windows.path.identity.changed = la identidad de la ruta de publicación cambió en el nivel conservado { $level }

journal.cli.describe.pipeline.windows.path.identity.mismatch = la identidad del ancestro '{ $component }' de la ruta de publicación cambió entre la creación y la reapertura

journal.cli.describe.pipeline.windows.path.interior.nul = la ruta de publicación contiene un { $nul } interior

journal.cli.describe.pipeline.windows.path.missing = el componente '{ $component }' de la ruta de publicación no existe

journal.cli.describe.pipeline.windows.path.move.spelling.diverged = la grafía de movimiento de la ruta de publicación ya no nombra el padre final

journal.cli.describe.pipeline.windows.path.not.directory = el ancestro '{ $component }' de la ruta de publicación no es un directorio

journal.cli.describe.pipeline.windows.path.not.regular.file = la hoja '{ $component }' de la ruta de publicación no es un archivo normal

journal.cli.describe.pipeline.windows.path.open.install.source = abrir el origen de la instalación: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.ancestor = abrir el ancestro de la ruta de publicación: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.anchor = abrir el ancla de la ruta de publicación: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.move.spelling = abrir la grafía de movimiento de la ruta de publicación: { $detail }

journal.cli.describe.pipeline.windows.path.path.too.long = la ruta de publicación supera el límite de longitud extendida

journal.cli.describe.pipeline.windows.path.query.install.path.attributes = consultar los atributos de la ruta de instalación: { $detail }

journal.cli.describe.pipeline.windows.path.query.install.source.identity = consultar la identidad del origen de la instalación: { $detail }

journal.cli.describe.pipeline.windows.path.query.publication.path.attributes = consultar los atributos de la ruta de publicación: { $detail }

journal.cli.describe.pipeline.windows.path.query.publication.path.identity = consultar la identidad de la ruta de publicación: { $detail }

journal.cli.describe.pipeline.windows.path.reopen.publication.path.ancestor = reabrir el ancestro de la ruta de publicación: { $detail }

journal.cli.describe.pipeline.windows.path.reparse.point = el ancestro '{ $component }' de la ruta de publicación es un punto de reanálisis

journal.cli.describe.pipeline.windows.path.resolve.publication.path.current.directory = resolver el directorio actual de la ruta de publicación: { $detail }

journal.cli.describe.pipeline.windows.path.revalidate.install.source.after.classification = revalidar el origen de la instalación después de la clasificación: { $detail }

journal.cli.describe.pipeline.windows.path.root.only = la ruta de publicación es una raíz de unidad sin destino

journal.cli.describe.pipeline.windows.path.rooted.without.drive = la ruta de publicación está enraizada sin unidad

journal.cli.describe.pipeline.windows.path.unc.or.device.prefix = la ruta de publicación tiene un prefijo { $unc } o de dispositivo

journal.cli.describe.pipeline.windows.path.unstable = la ruta de publicación no tiene un volumen { $guid } de { $manager }: { $detail }

journal.cli.describe.pipeline.withdraw = no se pudo preparar la retirada de { $ocr } de pantalla: { $detail }

journal.cli.describe.settings.finite = { $setting } debe ser un número finito

journal.cli.describe.settings.importance = { $setting } debe ser { $ignore }, { $low }, { $normal } o { $high }

journal.cli.describe.settings.object = { $setting } debe ser un objeto

journal.cli.describe.settings.string = { $setting } debe ser una cadena

journal.cli.describe.settings.strings = { $setting } debe ser un arreglo de cadenas

journal.cli.describe.settings.uint = { $setting } debe ser un entero de 32 bits sin signo

journal.cli.describe.stored.error.empty = Respuesta markdown vacía para { $category }

journal.cli.describe.stored.error.incomplete = La extracción nunca se completó

journal.cli.describe.stored.error.invalid = Respuesta { $format } no válida

journal.cli.describe.stored.error.invalid.category = Respuesta { $format } no válida para { $category }: { $error }

journal.cli.describe.stored.error.resize = no se pudo cambiar el tamaño del fotograma de extracción

journal.cli.describe.stored.error.schema = Falló la validación del esquema para { $category }

journal.cli.describe.stored.error.schema.detail = Falló la validación del esquema para { $category }: { $errors }

journal.cli.describe.stored.error.truncated = Respuesta markdown truncada para { $category }

journal.cli.describe.stored.note.bounded = [{ $mark }: salida limitada de la extracción antes de escribirla en el diario - longitud degenerada saneada/truncada]

journal.cli.describe.stored.note.ocr = texto reconocido de tus pantallas. puede contener errores de reconocimiento. esto dice lo que estaba visible, no lo que leíste, escribiste o hiciste. enmascarar contraseñas y otros secretos es de mejor esfuerzo; tus reglas de redacción no se aplican.

journal.cli.describe.stored.ocr.heading = { $clock } · { $category }

journal.cli.describe.usage.argument.unknown = uso: { $syntax }{ $line_break }{ $command }: error: argumento desconocido: { $argument }

journal.cli.describe.usage.backfill.arguments = uso: { $syntax }{ $line_break }{ $command }: error: el relleno de rostros requiere { $arguments }

journal.cli.describe.usage.backfill.budget = uso: { $syntax }{ $line_break }{ $command }: error: el presupuesto del relleno de rostros está fuera del rango permitido

journal.cli.describe.usage.backfill.desktop = uso: { $syntax }{ $line_break }{ $command }: error: el relleno de rostros solo está disponible en diarios de escritorio

journal.cli.describe.usage.describe.repeated = uso: { $syntax }{ $line_break }{ $command }: error: { $describe_flag } se proporcionó más de una vez

journal.cli.describe.usage.file.extra = uso: { $syntax }{ $line_break }{ $command }: error: solo se puede proporcionar una ruta de video

journal.cli.describe.usage.file.required = uso: { $syntax }{ $line_break }{ $command }: error: se requieren los siguientes argumentos: { $file }

journal.cli.describe.usage.frames.repeated = uso: { $syntax }{ $line_break }{ $command }: error: { $frames_only_flag } se proporcionó más de una vez

journal.cli.describe.usage.help = uso: { $syntax }

journal.cli.describe.usage.help.extra = uso: { $syntax }{ $line_break }{ $command }: error: { $help_flag } no acepta otros argumentos

journal.cli.describe.usage.jobs.positive = uso: { $syntax }{ $line_break }{ $command }: error: { $jobs_flag } requiere un entero positivo

journal.cli.describe.usage.jobs.repeated = uso: { $syntax }{ $line_break }{ $command }: error: { $jobs_flag } se proporcionó más de una vez

journal.cli.describe.usage.journal.path = uso: { $syntax }{ $line_break }{ $command }: error: { $journal_flag } requiere una ruta

journal.cli.describe.usage.journal.repeated = uso: { $syntax }{ $line_break }{ $command }: error: { $journal_flag } se proporcionó más de una vez

journal.cli.describe.usage.mode.required = uso: { $syntax }{ $line_break }{ $command }: error: se requiere exactamente uno de { $frames_only_flag } o { $describe_flag }

journal.cli.describe.usage.redo.mode = uso: { $syntax }{ $line_break }{ $command }: error: { $redo_flag } requiere { $describe_flag }

journal.cli.describe.usage.redo.repeated = uso: { $syntax }{ $line_break }{ $command }: error: { $redo_flag } se proporcionó más de una vez

journal.cli.describe.usage.registry.extra = uso: { $syntax }{ $line_break }{ $command }: error: { $category_registry_flag } no acepta otros argumentos

journal.cli.describe.usage.version.extra = uso: { $syntax }{ $line_break }{ $command }: error: { $version_flag } no acepta otros argumentos

journal.cli.grab.alternatives.days = Días disponibles (los 5 más cercanos):

journal.cli.grab.alternatives.screens = Pantallas disponibles en { $day }/{ $stream }/{ $segment }:

journal.cli.grab.alternatives.segments = Segmentos disponibles en { $day }/{ $stream }:

journal.cli.grab.alternatives.streams = Flujos disponibles en { $day }:

journal.cli.grab.error.captured.not.analyzed = la pantalla { $screen } en { $segment } está capturada pero no analizada

journal.cli.grab.error.day.not.found = no se encontró el día { $day }

journal.cli.grab.error.decode.failed.ids = no se pudieron decodificar los ids de fotograma: { $ids }

journal.cli.grab.error.decode.invalid.rgb = no se pudo decodificar el video: fotograma RGB no válido

journal.cli.grab.error.decode.no.video = no se pudo decodificar el video: no hay flujo de video

journal.cli.grab.error.decode.video = no se pudo decodificar el video: { $error }

journal.cli.grab.error.decoder.init = no se pudo inicializar el decodificador de video: { $error }

journal.cli.grab.error.force.requires.out = { $force_flag } requiere { $out_flag }

journal.cli.grab.error.frame.id.positive = los ids de fotograma deben ser enteros positivos: se obtuvo '{ $token }'

journal.cli.grab.error.frame.id.unique = los ids de fotograma deben ser únicos: { $frame_id }

journal.cli.grab.error.frame.not.found = no se encontró el id de fotograma { $frame_id } en { $screen } para { $segment }

journal.cli.grab.error.invalid.frame.id = id de fotograma no válido

journal.cli.grab.error.invalid.level4 = carga útil del nivel 4 no válida

journal.cli.grab.error.invalid.level5a = carga útil del nivel 5a no válida

journal.cli.grab.error.invalid.render.payload = carga útil de renderizado no válida

journal.cli.grab.error.invalid.segment.key = el segmento { $segment } no es una clave { $format } válida

journal.cli.grab.error.io = no se pudo acceder a { $path }: { $error }

journal.cli.grab.error.legacy.schema = el archivo de pantalla usa el esquema pre-frame_id; la selección de fotogramas no está disponible

journal.cli.grab.error.multiple.ids.require.out = varios ids de fotograma requieren { $out_flag }

journal.cli.grab.error.out.exists = la ruta de salida existe (usa { $force_flag }): { $paths }

journal.cli.grab.error.out.extension = { $out_flag } debe terminar en { $extensions }

journal.cli.grab.error.out.filename = la ruta de salida no tiene un nombre de archivo válido

journal.cli.grab.error.out.requires.five.tokens = { $out_flag } requiere day stream segment screen y frame-id

journal.cli.grab.error.save.image = no se pudo guardar la imagen: { $error }

journal.cli.grab.error.screen.not.found = no se encontró la pantalla { $screen } en { $day }/{ $stream }/{ $segment }

journal.cli.grab.error.segment.not.found = no se encontró el segmento { $segment } en { $day }/{ $stream }

journal.cli.grab.error.stream.not.found = no se encontró el flujo { $stream } en { $day }

journal.cli.grab.error.timestamp.finite = la marca de tiempo debe ser finita

journal.cli.grab.error.timestamp.invalid = marca de tiempo no válida

journal.cli.grab.error.timestamp.range = la marca de tiempo está fuera de rango

journal.cli.grab.error.too.many.tokens = grab acepta como máximo 5 tokens posicionales: day stream segment screen frame-id

journal.cli.grab.error.unsupported.level = nivel de salida no admitido

journal.cli.grab.error.unsupported.level.named = nivel de salida no admitido: { $level }

journal.cli.grab.error.video.missing = no se encontró el video en bruto para la pantalla { $screen } en { $segment }

journal.cli.grab.error.video.missing.with.reason = { $reason }; el acceso solo a metadatos sigue disponible mediante: { $command }

journal.cli.grab.frame.note.error = error: { $text }

journal.cli.grab.header.abs.time = abs_time

journal.cli.grab.header.connector = conector

journal.cli.grab.header.day = día

journal.cli.grab.header.end = fin

journal.cli.grab.header.frame.id = frame_id

journal.cli.grab.header.frames.analyzed = frames_analyzed

journal.cli.grab.header.notes = notas

journal.cli.grab.header.position = posición

journal.cli.grab.header.primary = principal

journal.cli.grab.header.screen = pantalla

journal.cli.grab.header.screens = pantallas

journal.cli.grab.header.segment = segmento

journal.cli.grab.header.segments = segmentos

journal.cli.grab.header.start = inicio

journal.cli.grab.header.status = estado

journal.cli.grab.header.stream = flujo

journal.cli.grab.header.streams = flujos

journal.cli.grab.header.timestamp = marca de tiempo

journal.cli.grab.level4.empty = No hay fotogramas calificados en el análisis de esta pantalla.

journal.cli.grab.level4.footer = Inspeccionar:   { $inspect_command }{ $line_break }Guardar uno:    { $save_one_command }{ $line_break }Guardar varios: { $save_many_command }{ $line_break }{ $line_break }Cómo funciona la extracción:{ $line_break }  La decodificación recorre el video de forma lineal desde el fotograma 0 — saltar es inseguro a la{ $line_break }  tasa de captura de 1 Hz. El costo lo domina el frame_id solicitado más alto, no{ $line_break }  la cantidad. Pedir los ids 7,12,23 cuesta lo mismo que pedir solo el 23.{ $line_break }  Prefiere el modo por lotes cuando quieres más de un fotograma de la misma pantalla.

journal.cli.grab.level4.legacy = 0 fotogramas analizados: el archivo usa el esquema pre-frame_id

journal.cli.grab.level4.missing.video = El modo de guardado no está disponible: { $reason }.{ $line_break }Los metadatos de fotogramas de arriba siguen siendo legibles.{ $line_break }{ $line_break }Inspeccionar: { $command }

journal.cli.grab.level5a.footer = Guardar: { $save_command }{ $line_break }Lote: { $batch_command }

journal.cli.grab.level5a.frame = Fotograma: { $value }

journal.cli.grab.level5a.jsonl = JSONL: { $value }

journal.cli.grab.level5a.notes = Notas: { $value }

journal.cli.grab.level5a.screen = Pantalla: { $value }

journal.cli.grab.level5a.time = Hora: { $value }

journal.cli.grab.level5a.video = Video: { $value }

journal.cli.grab.missing.reason.offload = eliminaste el video original después de que tu copia de seguridad lo copió

journal.cli.grab.missing.reason.owner = eliminaste el video original

journal.cli.grab.missing.reason.policy = eliminaste el video original después de que tu configuración de retención lo marcó

journal.cli.grab.missing.reason.trash = el video original está en la papelera, donde puedes recuperarlo en la configuración

journal.cli.grab.missing.reason.unknown = el video original ya no está en tu diario

journal.cli.grab.next = Siguiente: { $command }

journal.cli.grab.saved = se guardó { $path }

journal.cli.grab.status.analyzed = analizada

journal.cli.grab.status.captured = capturada pero no analizada

journal.cli.grab.status.missing.offload = analizada; eliminaste el video original después de que tu copia de seguridad lo copió

journal.cli.grab.status.missing.owner = analizada; eliminaste el video original

journal.cli.grab.status.missing.policy = analizada; eliminaste el video original después de que tu configuración de retención lo marcó

journal.cli.grab.status.missing.trash = analizada; el video original está en la papelera

journal.cli.grab.status.missing.unknown = analizada; el video original ya no está en tu diario

journal.cli.maintenance.backup.erase = borrado de copia de seguridad: { $state } { $removed_field }{ $removed } { $shared_field }{ $shared } { $staying_field }{ $staying } { $unreached_field }{ $unreached } { $pending_field }{ $pending }

journal.cli.maintenance.backup.erase.error.detail = borrado de copia de seguridad: error { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.erase.reason = borrado de copia de seguridad: { $state } { $removed_field }{ $removed } { $shared_field }{ $shared } { $staying_field }{ $staying } { $unreached_field }{ $unreached } { $pending_field }{ $pending } { $reason_field }{ $reason }

journal.cli.maintenance.backup.error = copia de seguridad: error { $reason_field }{ $reason }

journal.cli.maintenance.backup.error.detail = copia de seguridad: error { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.ok = copia de seguridad: ok { $field }{ $snapshot }

journal.cli.maintenance.backup.paused = copia de seguridad: omitida. está en pausa, así que no se ejecutó nada. ejecuta `{ $resume }` para reanudarla.

journal.cli.maintenance.backup.prune.error = poda de copia de seguridad: error { $reason_field }{ $reason }

journal.cli.maintenance.backup.prune.error.detail = poda de copia de seguridad: error { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.prune.ok = poda de copia de seguridad: ok

journal.cli.maintenance.backup.prune.paused = poda de copia de seguridad: omitida. está en pausa, así que no se ejecutó nada. ejecuta `{ $resume }` para reanudarla.

journal.cli.maintenance.backup.prune.skipped = poda de copia de seguridad: omitida

journal.cli.maintenance.backup.skipped = copia de seguridad: omitida

journal.cli.maintenance.backup.superseded = copia de seguridad: la reclamó otro dispositivo; se borró la configuración local. ejecuta `{ $enable }` para configurar una nueva aquí.

journal.cli.maintenance.backup.unavailable = no están disponibles servicios de mantenimiento de la copia de seguridad.

journal.cli.maintenance.backup.usage.offload = uso: { $command } { $id }{ $options }{ $line_break }{ $command } { $id }: error: argumentos no reconocidos: { $args }

journal.cli.maintenance.backup.usage.routine = uso: { $command } { $id }{ $options }{ $line_break }{ $command } { $id }: error: argumentos no reconocidos: { $args }

journal.cli.maintenance.backup.verify.error = verificación de copia de seguridad: error { $reason_field }{ $reason }

journal.cli.maintenance.backup.verify.error.detail = verificación de copia de seguridad: error { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.verify.ok = verificación de copia de seguridad: ok { $field }{ $subset }

journal.cli.maintenance.backup.verify.paused = verificación de copia de seguridad: omitida. está en pausa, así que no se ejecutó nada. ejecuta `{ $resume }` para reanudarla.

journal.cli.maintenance.backup.verify.skipped = verificación de copia de seguridad: omitida

journal.cli.maintenance.description.backup.erase = quitar también de tus copias de seguridad lo que borraste, cuando lo elegiste.

journal.cli.maintenance.description.backup.offload = trasladar el contenido multimedia en bruto verificado después de la copia de seguridad.

journal.cli.maintenance.description.backup.prune = aplicar la política de retención de la copia de seguridad cifrada.

journal.cli.maintenance.description.backup.run = ejecutar la copia de seguridad cifrada.

journal.cli.maintenance.description.backup.verify = verificar la relectura de la copia de seguridad cifrada.

journal.cli.maintenance.description.faces.backfill = actualizar las sugerencias de foto de perfil a partir de reuniones, incluidas las reuniones pasadas.

journal.cli.maintenance.description.health.mark.raw = listar el contenido multimedia original listo para quitar.

journal.cli.maintenance.description.health.prune.logs = podar registros operativos antiguos.

journal.cli.maintenance.description.retention.owner.voice.originals = terminar de eliminar el audio original de las grabaciones recientes procesadas con "dejar fuera las otras voces" activado.

journal.cli.maintenance.description.speakers.backfill.labels = nombrar a los hablantes, sin el modelo, en el audio que todavía no tiene etiquetas de hablante.

journal.cli.maintenance.description.speakers.backfill.pool = poner al día la lista de voces a partir del audio ya transcrito y luego buscar tu voz.

journal.cli.maintenance.description.speakers.candidate.pair.suggestions = Encuentra pares densos de candidatos a hablante para Sugerencias.

journal.cli.maintenance.description.speakers.consolidate.pool = Consolida candidatos densos a hablante.

journal.cli.maintenance.description.speakers.discover.voices = Actualiza la caché de descubrimiento de voces recurrentes.

journal.cli.maintenance.description.speakers.name.variants = Encuentra variantes de nombre de hablante para Sugerencias.

journal.cli.maintenance.description.trash.erase = terminar los pasos de la papelera que una detención interrumpió y borrar lo que ya esperó sus días.

journal.cli.maintenance.face.generation = no se admitió la generación del relleno de rostros

journal.cli.maintenance.face.log.cache = no se guardó la caché de escaneo del relleno de rostros: { $detail }

journal.cli.maintenance.face.log.deferred = se aplazó la recuperación de participación de rostros: { $detail }

journal.cli.maintenance.face.log.failed = no se pudieron rellenar los rostros: { $detail }

journal.cli.maintenance.face.priority = la prioridad del historial de rostros no está disponible: { $error }

journal.cli.maintenance.face.reconciliation = conciliación de rostros: { $error }

journal.cli.maintenance.health.config.days = { $field } debe ser un entero positivo

journal.cli.maintenance.health.config.enabled = { $field } debe ser un booleano

journal.cli.maintenance.health.keep = { $routine }: tu configuración de retención conserva todo el contenido multimedia original.

journal.cli.maintenance.health.mark.entry = { $indent }{ $id }: { $reason }

journal.cli.maintenance.health.mark.refused = { $routine }: no se pudieron listar algunos elementos:

journal.cli.maintenance.health.mark.summary = { $routine }: elementos nuevos: { $new }.{ $line_break }  total acumulado: { $total }.

journal.cli.maintenance.health.mark.unavailable = { $routine }: no se pudo construir la lista: { $error }

journal.cli.maintenance.health.prune.args.integer = uso: { $command } { $syntax }{ $line_break }{ $command }: error: argumento { $flag }: valor entero no válido: '{ $value }'

journal.cli.maintenance.health.prune.args.missing = uso: { $command } { $syntax }{ $line_break }{ $command }: error: argumento { $flag }: se esperaba un argumento

journal.cli.maintenance.health.prune.args.positive = { $routine }: { $flag } debe ser un entero positivo

journal.cli.maintenance.health.prune.args.unexpected = uso: { $command } { $syntax }{ $line_break }{ $command }: error: argumentos no reconocidos: { $value }

journal.cli.maintenance.health.prune.class = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $indent }{ $class }: { $files } archivo, { $dirs } directorio de caché, { $bytes }
        [many] { $indent }{ $class }: { $files } archivo, { $dirs } directorios de caché, { $bytes }
       *[other] { $indent }{ $class }: { $files } archivo, { $dirs } directorios de caché, { $bytes }
    }
    [many] { $dirs_quantity ->
        [one] { $indent }{ $class }: { $files } archivos, { $dirs } directorio de caché, { $bytes }
        [many] { $indent }{ $class }: { $files } archivos, { $dirs } directorios de caché, { $bytes }
       *[other] { $indent }{ $class }: { $files } archivos, { $dirs } directorios de caché, { $bytes }
    }
   *[other] { $dirs_quantity ->
        [one] { $indent }{ $class }: { $files } archivos, { $dirs } directorio de caché, { $bytes }
        [many] { $indent }{ $class }: { $files } archivos, { $dirs } directorios de caché, { $bytes }
       *[other] { $indent }{ $class }: { $files } archivos, { $dirs } directorios de caché, { $bytes }
    }
}

journal.cli.maintenance.health.prune.disabled = { $routine }: desactivado

journal.cli.maintenance.health.prune.halted = { $routine }: { $message }

journal.cli.maintenance.health.prune.missing = el directorio del diario no está disponible

journal.cli.maintenance.health.prune.preview = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $routine }: podaría { $files } archivo de registro operativo, { $dirs } directorio de caché, { $bytes } { $cutoff_field }{ $cutoff }
        [many] { $routine }: podaría { $files } archivo de registro operativo, { $dirs } directorios de caché, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: podaría { $files } archivo de registro operativo, { $dirs } directorios de caché, { $bytes } { $cutoff_field }{ $cutoff }
    }
    [many] { $dirs_quantity ->
        [one] { $routine }: podaría { $files } archivos de registro operativo, { $dirs } directorio de caché, { $bytes } { $cutoff_field }{ $cutoff }
        [many] { $routine }: podaría { $files } archivos de registro operativo, { $dirs } directorios de caché, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: podaría { $files } archivos de registro operativo, { $dirs } directorios de caché, { $bytes } { $cutoff_field }{ $cutoff }
    }
   *[other] { $dirs_quantity ->
        [one] { $routine }: podaría { $files } archivos de registro operativo, { $dirs } directorio de caché, { $bytes } { $cutoff_field }{ $cutoff }
        [many] { $routine }: podaría { $files } archivos de registro operativo, { $dirs } directorios de caché, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: podaría { $files } archivos de registro operativo, { $dirs } directorios de caché, { $bytes } { $cutoff_field }{ $cutoff }
    }
}

journal.cli.maintenance.health.prune.queue = se quitó un registro, pero no se pudo poner en cola su actualización de búsqueda: { $error }

journal.cli.maintenance.health.prune.refused = { $routine }: no se pudieron podar algunos registros: { $summary }

journal.cli.maintenance.health.prune.refused.unnamed = { $routine }: no se pudieron podar algunos registros: la herramienta de retención rechazó sin nombrar una entrada.

journal.cli.maintenance.health.prune.result = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $routine }: podó { $files } archivo de registro operativo, { $dirs } directorio de caché, { $bytes } { $cutoff_field }{ $cutoff }
        [many] { $routine }: podó { $files } archivo de registro operativo, { $dirs } directorios de caché, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: podó { $files } archivo de registro operativo, { $dirs } directorios de caché, { $bytes } { $cutoff_field }{ $cutoff }
    }
    [many] { $dirs_quantity ->
        [one] { $routine }: podó { $files } archivos de registro operativo, { $dirs } directorio de caché, { $bytes } { $cutoff_field }{ $cutoff }
        [many] { $routine }: podó { $files } archivos de registro operativo, { $dirs } directorios de caché, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: podó { $files } archivos de registro operativo, { $dirs } directorios de caché, { $bytes } { $cutoff_field }{ $cutoff }
    }
   *[other] { $dirs_quantity ->
        [one] { $routine }: podó { $files } archivos de registro operativo, { $dirs } directorio de caché, { $bytes } { $cutoff_field }{ $cutoff }
        [many] { $routine }: podó { $files } archivos de registro operativo, { $dirs } directorios de caché, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: podó { $files } archivos de registro operativo, { $dirs } directorios de caché, { $bytes } { $cutoff_field }{ $cutoff }
    }
}

journal.cli.maintenance.health.prune.unavailable = { $routine }: no se pudieron podar los registros: { $error }

journal.cli.maintenance.health.prune.undateable = { $indent }error: no se pudo determinar la fecha de retención de la entrada: no se pudo determinar la fecha de retención de la entrada

journal.cli.maintenance.health.prune.unreadable = { $indent }error: { $detail }: { $detail }

journal.cli.maintenance.health.unavailable = los servicios de mantenimiento del estado no están disponibles.

journal.cli.maintenance.health.usage = uso: { $command } { $id } { $syntax }{ $line_break }{ $command } { $id }: error: argumentos no reconocidos: { $args }

journal.cli.maintenance.help.list = uso: { $syntax }

journal.cli.maintenance.help.root = uso: { $syntax }

journal.cli.maintenance.help.run = uso: { $syntax }

journal.cli.maintenance.help.sync = uso: { $syntax }

journal.cli.maintenance.list.empty = No se encontraron rutinas de mantenimiento.

journal.cli.maintenance.list.heading.description = DESCRIPCIÓN

journal.cli.maintenance.list.heading.every = CADA

journal.cli.maintenance.list.heading.id = ID

journal.cli.maintenance.list.heading.runtime = TIEMPO MÁXIMO EN EJECUCIÓN

journal.cli.maintenance.list.heading.status = ESTADO

journal.cli.maintenance.list.status.divergent = divergente

journal.cli.maintenance.list.status.missing = ausente

journal.cli.maintenance.list.status.nothere = n/d

journal.cli.maintenance.list.status.synced = sincronizado

journal.cli.maintenance.missing.root = uso: { $syntax }{ $line_break }{ $command }: error: se requieren los siguientes argumentos: { $argument }

journal.cli.maintenance.missing.run = uso: { $syntax }{ $line_break }{ $command }: error: se requieren los siguientes argumentos: { $argument }

journal.cli.maintenance.retention.import.owed = limpieza del audio importado incompleta: { $error }

journal.cli.maintenance.retention.owed = limpieza del original de la voz propia incompleta: { $error }

journal.cli.maintenance.retention.summary = { $routine }: eliminación { $deleted }, eliminación sin registro { $deleted_unrecorded }, ya ausentes { $gone }, todavía pendientes { $owed }, conservación definitiva { $refused }, no verificables { $not_provable }, nada pendiente { $not_owed }, omisión por ocupado { $skipped }, eventos ilegibles { $unknown }

journal.cli.maintenance.retention.usage = uso: { $id } no admite argumentos.

journal.cli.maintenance.routine.unimplemented = { $count_quantity ->
    [one] la rutina de mantenimiento { $id } todavía no está implementada ({ $count } argumento reenviado).
    [many] la rutina de mantenimiento { $id } todavía no está implementada ({ $count } argumentos reenviados).
   *[other] la rutina de mantenimiento { $id } todavía no está implementada ({ $count } argumentos reenviados).
}

journal.cli.maintenance.routine.unknown = Rutina de mantenimiento desconocida: { $id }. Ejecuta `{ $command }` para ver las rutinas disponibles.

journal.cli.maintenance.schedule.error = Error al leer o actualizar { $path }: { $error } (causa: { $error })

journal.cli.maintenance.schedule.io = no se pudo completar la E/S de la programación: { $detail }

journal.cli.maintenance.schedule.malformed = configuración de programaciones mal formada en { $path }

journal.cli.maintenance.schedule.metadata = claves de metadatos de programación desconocidas: { $keys }

journal.cli.maintenance.schedule.shape = { $path }: la configuración de programaciones debe ser un objeto JSON

journal.cli.maintenance.schedule.state.shape = el estado de la programación en { $path } debe ser un objeto JSON

journal.cli.maintenance.speakers.discovery.helper = no se pudo ejecutar el ayudante de descubrimiento de hablantes en { $stage }: { $reason }

journal.cli.maintenance.speakers.discovery.input = no se pudo completar el descubrimiento de hablantes: { $detail }

journal.cli.maintenance.speakers.error.owner.voice.look.failed = no se pudo buscar la voz propia: { $error }

journal.cli.maintenance.speakers.error.speaker.candidate.consolidation.failed = no se pudieron consolidar los candidatos a hablante: { $error }

journal.cli.maintenance.speakers.error.speaker.candidate.pair.refresh.failed = no se pudieron actualizar los pares de candidatos a hablante: { $error }

journal.cli.maintenance.speakers.error.speaker.label.backfill.failed = no se pudieron rellenar las etiquetas de hablante: { $error }

journal.cli.maintenance.speakers.error.speaker.name.variant.refresh.failed = no se pudieron actualizar las variantes de nombre de hablante: { $error }

journal.cli.maintenance.speakers.not.admitted = no se admitió la generación del descubrimiento de hablantes al entrar en el mantenimiento

journal.cli.maintenance.speakers.usage.backfill.labels = uso: { $command }

journal.cli.maintenance.speakers.usage.backfill.pool = uso: { $command }

journal.cli.maintenance.speakers.usage.candidate.pair.suggestions = uso: { $command }

journal.cli.maintenance.speakers.usage.consolidate.pool = uso: { $command }

journal.cli.maintenance.speakers.usage.discover.voices = uso: { $command }

journal.cli.maintenance.speakers.usage.name.variants = uso: { $command }

journal.cli.maintenance.sync.added = se agregaron: { $count }: { $ids }

journal.cli.maintenance.sync.added.empty = se agregaron: 0

journal.cli.maintenance.sync.disabled = se desactivaron: { $count }: { $ids }

journal.cli.maintenance.sync.disabled.empty = se desactivaron: 0

journal.cli.maintenance.sync.divergent = en divergencia: { $count }: { $ids }

journal.cli.maintenance.sync.divergent.empty = en divergencia: 0

journal.cli.maintenance.sync.removed = se quitaron: { $count }: { $ids }

journal.cli.maintenance.sync.removed.empty = se quitaron: 0

journal.cli.maintenance.sync.synced = se sincronizaron: { $count }: { $ids }

journal.cli.maintenance.sync.synced.empty = se sincronizaron: 0

journal.cli.maintenance.sync.warning.disabled = ADVERTENCIA: la programación { $id } está desactivada; se conservó desactivada

journal.cli.maintenance.sync.warning.divergent = ADVERTENCIA: la programación { $id } es divergente; se conservó sin cambios

journal.cli.maintenance.trash.erased = papelera: se borraron { $erased }, se terminaron { $finished }, en espera { $waiting }, fallas { $failed }

journal.cli.maintenance.trash.error = papelera: { $failure }

journal.cli.maintenance.trash.usage = uso: { $id } no admite argumentos.

journal.cli.maintenance.usage.face = uso: { $syntax }{ $line_break }{ $command }: error: argumentos no reconocidos: argumentos inesperados de conciliación de rostros

journal.cli.maintenance.usage.unknown = uso: { $syntax }{ $line_break }{ $command }: error: argumentos no reconocidos: { $arguments }

journal.cli.offload.guidance.changed.retry = el traslado se detuvo porque tu diario cambió mientras se ejecutaba. no se quitó nada de esta computadora. vuelve a ejecutarlo.

journal.cli.offload.guidance.changed.review = el traslado se detuvo porque tu diario cambió mientras se ejecutaba. no se quitó nada de esta computadora. conserva sus archivos de recuperación y revisa los efectos registrados antes de volver a intentarlo.

journal.cli.offload.guidance.clock = no se pudo confirmar el reloj de la operación.

journal.cli.offload.guidance.journal = no se pudo confirmar tu diario. conserva sus archivos de recuperación para revisión.

journal.cli.offload.guidance.platform = la ejecución del traslado no está disponible en esta plataforma.

journal.cli.offload.guidance.start = el traslado no se inició.

journal.cli.offload.guidance.tool = usa el comando { $solstone } instalado con su herramienta de copia de seguridad empaquetada.

journal.cli.offload.guidance.unfinished = el traslado no terminó. conserva sus archivos de recuperación y revisa los efectos registrados antes de volver a intentarlo.

journal.cli.offload.result.archive.failed = no se pudo completar el paso de archivado del traslado

journal.cli.offload.result.backup.failing = tu última copia de seguridad no se pudo completar con éxito

journal.cli.offload.result.backup.not.ready = tu copia de seguridad no está configurada o todavía no completó una ejecución con éxito

journal.cli.offload.result.budget.already.satisfied = el presupuesto del traslado ya se cumplió, así que no hizo falta marcar nada para liberar

journal.cli.offload.result.budget.satisfied = se marcó suficiente contenido multimedia para liberar y cumplir el presupuesto del traslado

journal.cli.offload.result.confirm.failed = no se pudo confirmar en el traslado que los archivos archivados llegaran realmente a la copia de seguridad

journal.cli.offload.result.confirm.tool.failed = no se pudo completar la revisión que confirma un archivo de la copia del traslado

journal.cli.offload.result.markable.media.exhausted = ya se revisó cada archivo elegible; ahora no se puede marcar nada más para liberar

journal.cli.offload.result.other = se detuvo sin un motivo ({ $reason })

journal.cli.offload.result.unexpected.error = algo inesperado interrumpió el traslado

journal.cli.offload.result.verification.failed = la última verificación de la copia de seguridad encontró un problema con su integridad

journal.cli.offload.result.verification.missing = todavía no se completó ninguna verificación de la copia de seguridad

journal.cli.offload.result.verification.overdue = la verificación de la copia de seguridad no tuvo éxito lo bastante reciente

journal.cli.offload.stopped.archive.cancelled = el traslado se detuvo: se canceló la operación de archivado.

journal.cli.offload.stopped.archive.clock.changed = el traslado se detuvo: el reloj del archivado retrocedió.

journal.cli.offload.stopped.archive.configuration.changed = el traslado se detuvo: cambió la configuración del archivado.

journal.cli.offload.stopped.archive.configuration.unavailable = el traslado se detuvo: no se pudo leer la configuración del archivado.

journal.cli.offload.stopped.archive.credentials.expired = el traslado se detuvo: las credenciales del archivado vencieron.

journal.cli.offload.stopped.archive.deadline.exceeded = el traslado se detuvo: se alcanzó el plazo de la operación de archivado.

journal.cli.offload.stopped.archive.invalid.credential.expiry = el traslado se detuvo: las credenciales del archivado tienen un vencimiento no válido.

journal.cli.offload.stopped.archive.invalid.selection = el traslado se detuvo: las fuentes del archivado deben ser una selección no vacía de rutas absolutas.

journal.cli.offload.stopped.archive.journal.identity.changed = el traslado se detuvo: no se pudo confirmar tu diario.

journal.cli.offload.stopped.archive.not.configured = el traslado se detuvo: la copia de seguridad del archivado no está configurada.

journal.cli.offload.stopped.archive.repository.identity.changed = el traslado se detuvo: la copia de seguridad ya no coincide.

journal.cli.offload.stopped.archive.repository.identity.invalid = el traslado se detuvo: no se pudo identificar la copia de seguridad.

journal.cli.offload.stopped.archive.snapshot.identity.invalid = el traslado se detuvo: el archivado no devolvió una identidad completa de la instantánea.

journal.cli.offload.stopped.archive.source.unavailable = el traslado se detuvo: no se pudieron confirmar los archivos preparados del archivado.

journal.cli.offload.stopped.archive.tool.unavailable = el traslado se detuvo: una herramienta de archivado no está disponible.

journal.cli.offload.stopped.creation = el traslado se detuvo: no terminó la creación de propuestas.

journal.cli.offload.stopped.inputs.io = el traslado se detuvo: no se pudo abrir o leer un archivo de tu diario.

journal.cli.offload.stopped.inputs.space = el traslado se detuvo: no hay suficiente espacio libre en esta computadora ({ $available } de espacio libre, se necesitan { $required }).

journal.cli.offload.stopped.native.start = el traslado se detuvo: se rechazó el inicio del archivado (estado { $status }).

journal.cli.offload.stopped.plan.incomplete = el traslado se detuvo: no terminó la preparación de propuestas.

journal.cli.offload.stopped.planning = el traslado se detuvo: no se pudo preparar sus propuestas.

journal.cli.offload.stopped.readback = traslado detenido: no se pudo leer la copia de vuelta desde tu copia de seguridad.

journal.cli.offload.stopped.readback.credentials.unavailable = traslado detenido: no hay credenciales para leer el archivo de la copia.

journal.cli.offload.stopped.readback.incomplete.discovery = traslado detenido: el recorrido del archivo de la copia no contabilizó todos los nodos.

journal.cli.offload.stopped.readback.invalid.selection = traslado detenido: el archivo de la copia seleccionado no es válido.

journal.cli.offload.stopped.readback.native.failure = traslado detenido: no se pudo preparar el archivo archivado.

journal.cli.offload.stopped.readback.not.configured = traslado detenido: leer archivos archivados no está configurado.

journal.cli.offload.stopped.readback.snapshot.node.unavailable = traslado detenido: no se pudo confirmar el archivo archivado.

journal.cli.offload.stopped.readback.target.root.changed = traslado detenido: no se pudo preparar el archivo en tu diario.

journal.cli.offload.stopped.readback.tool.changed = traslado detenido: cambió la herramienta de archivado instalada; vuelve a intentarlo.

journal.cli.offload.stopped.readback.tool.failed = traslado detenido: no se pudo completar la lectura del archivo de la copia.

journal.cli.offload.stopped.readback.tool.unavailable = traslado detenido: la herramienta de archivado instalada no está disponible.

journal.cli.offload.stopped.readback.unsupported = traslado detenido: leer archivos archivados no está disponible en esta plataforma.

journal.cli.offload.stopped.readback.unsupported.backend = traslado detenido: este destino de copia de seguridad no admite leer archivos archivados.

journal.cli.offload.stopped.readback.unsupported.tool.version = traslado detenido: la versión de la herramienta de archivado instalada no es compatible.

journal.cli.offload.stopped.record = traslado detenido: no se pudo guardar el registro del traslado.

journal.cli.offload.stopped.selection = traslado detenido: no se pudieron confirmar las grabaciones seleccionadas.

journal.cli.offload.stopped.temporary.copy = traslado detenido: no se pudo hacer una copia temporal.

journal.cli.offload.stopped.tool.reason = traslado detenido: { $reason }.

journal.cli.offload.stopped.unfinished.work = traslado detenido: tu diario todavía tiene trabajo sin terminar.

journal.cli.reprocess.empty.range = no hay datos de los días { $day } hasta { $through }

journal.cli.reprocess.facet.classification = { $path }: clasificación de faceta no válida

journal.cli.reprocess.facet.utf8 = faceta no { $encoding }

journal.cli.reprocess.help = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $line_break }Envía un día pasado del diario para volver a procesarlo{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $day_column }Día pasado en formato { $date_format }{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_9 }muestra este mensaje de ayuda y sale{ $line_break }{ $indent_2 }{ $through_option }{ $gap_2 }Fin inclusivo del rango en formato { $date_format }{ $line_break }{ $syntax_continuation_13 }{ $line_break }{ $indent_2 }{ $unit_option }{ $gap_8 }Pide un reintento de un paso del procesamiento diario que no se pudo completar{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_6 }Faceta de esa unidad; obligatoria cuando la unidad tiene una faceta{ $line_break }{ $indent_2 }{ $from_scratch_option }{ $gap_5 }Fuerza una repetición diaria completa, conservando los marcadores (no{ $line_break }                     marca el día como actualizado){ $line_break }{ $indent_2 }{ $mark_updated_option }{ $gap_5 }Marca el día con datos en bruto nuevos para que el procesamiento diario{ $line_break }                     lo vuelva a poner en cola, y luego impulsa un drenado{ $line_break }{ $indent_2 }{ $owed_option }{ $gap_13 }Lista las salidas diarias pendientes del día o del rango y por qué,{ $line_break }                     sin enviar nada{ $line_break }{ $indent_2 }{ $json_option }{ $gap_13 }Con { $owed_flag }, informa cada día como estados y conteos: sus{ $line_break }                     marcadores, su cobertura, cuántas salidas diarias tiene y{ $line_break }                     cuántas están pendientes, por causa{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_6 }Activa la salida detallada{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_8 }Activa el registro de depuración

journal.cli.reprocess.input.day = día no válido: { $day }

journal.cli.reprocess.intent.busy = otro ejecutor tiene el día solicitado

journal.cli.reprocess.intent.invalid.day = día de trabajo no válido

journal.cli.reprocess.intent.uncertain = la publicación de la intención de trabajo es incierta: { $outcome }

journal.cli.reprocess.intent.unrecognized = intención de trabajo no reconocida

journal.cli.reprocess.native.arguments = no se pudo completar { $binary } nativo { $verb }: los argumentos no son un { $encoding } válido

journal.cli.reprocess.no.data = no hay datos del día { $day }

journal.cli.reprocess.no.engine = todavía no hay un modelo elegido. elige uno en razonamiento y vuelve a intentarlo

journal.cli.reprocess.owed.internal = { $flag } solo informa; usa { $function }

journal.cli.reprocess.owed.only = { $owed_flag } solo informa; no se envió nada

journal.cli.reprocess.past.only = reprocess solo admite el pasado (no se puede volver a procesar hoy ni un día futuro)

journal.cli.reprocess.path.utf8 = ruta de fuente no { $encoding }

journal.cli.reprocess.range.plan = { $days_quantity ->
    [one] { $segments_quantity ->
        [one] plan de reprocess from-scratch:{ $break }{ $days } día con datos ({ $segments } segmento) quedará en cola. El progreso se verá en { $top } o en { $health }. Las solicitudes sobreviven a un reinicio del supervisor; un día que no puede terminar desde cero termina como un reprocess ordinario.{ $break }Este día puede tardar horas; el trabajo de hoy va primero entre tareas.{ $break }vuelve a ejecutarlo con { $yes } para continuar
        [many] plan de reprocess from-scratch:{ $break }{ $days } día con datos ({ $segments } segmentos) quedará en cola. El progreso se verá en { $top } o en { $health }. Las solicitudes sobreviven a un reinicio del supervisor; un día que no puede terminar desde cero termina como un reprocess ordinario.{ $break }Este día puede tardar horas; el trabajo de hoy va primero entre tareas.{ $break }vuelve a ejecutarlo con { $yes } para continuar
       *[other] plan de reprocess from-scratch:{ $break }{ $days } día con datos ({ $segments } segmentos) quedará en cola. El progreso se verá en { $top } o en { $health }. Las solicitudes sobreviven a un reinicio del supervisor; un día que no puede terminar desde cero termina como un reprocess ordinario.{ $break }Este día puede tardar horas; el trabajo de hoy va primero entre tareas.{ $break }vuelve a ejecutarlo con { $yes } para continuar
    }
    [many] { $segments_quantity ->
        [one] plan de reprocess from-scratch:{ $break }{ $days } días con datos ({ $segments } segmento) quedarán en cola. El progreso se verá en { $top } o en { $health }. Las solicitudes sobreviven a un reinicio del supervisor; un día que no puede terminar desde cero termina como un reprocess ordinario.{ $break }Estos días pueden tardar horas; el trabajo de hoy va primero entre tareas.{ $break }vuelve a ejecutarlo con { $yes } para continuar
        [many] plan de reprocess from-scratch:{ $break }{ $days } días con datos ({ $segments } segmentos) quedarán en cola. El progreso se verá en { $top } o en { $health }. Las solicitudes sobreviven a un reinicio del supervisor; un día que no puede terminar desde cero termina como un reprocess ordinario.{ $break }Estos días pueden tardar horas; el trabajo de hoy va primero entre tareas.{ $break }vuelve a ejecutarlo con { $yes } para continuar
       *[other] plan de reprocess from-scratch:{ $break }{ $days } días con datos ({ $segments } segmentos) quedarán en cola. El progreso se verá en { $top } o en { $health }. Las solicitudes sobreviven a un reinicio del supervisor; un día que no puede terminar desde cero termina como un reprocess ordinario.{ $break }Estos días pueden tardar horas; el trabajo de hoy va primero entre tareas.{ $break }vuelve a ejecutarlo con { $yes } para continuar
    }
   *[other] { $segments_quantity ->
        [one] plan de reprocess from-scratch:{ $break }{ $days } días con datos ({ $segments } segmento) quedarán en cola. El progreso se verá en { $top } o en { $health }. Las solicitudes sobreviven a un reinicio del supervisor; un día que no puede terminar desde cero termina como un reprocess ordinario.{ $break }Estos días pueden tardar horas; el trabajo de hoy va primero entre tareas.{ $break }vuelve a ejecutarlo con { $yes } para continuar
        [many] plan de reprocess from-scratch:{ $break }{ $days } días con datos ({ $segments } segmentos) quedarán en cola. El progreso se verá en { $top } o en { $health }. Las solicitudes sobreviven a un reinicio del supervisor; un día que no puede terminar desde cero termina como un reprocess ordinario.{ $break }Estos días pueden tardar horas; el trabajo de hoy va primero entre tareas.{ $break }vuelve a ejecutarlo con { $yes } para continuar
       *[other] plan de reprocess from-scratch:{ $break }{ $days } días con datos ({ $segments } segmentos) quedarán en cola. El progreso se verá en { $top } o en { $health }. Las solicitudes sobreviven a un reinicio del supervisor; un día que no puede terminar desde cero termina como un reprocess ordinario.{ $break }Estos días pueden tardar horas; el trabajo de hoy va primero entre tareas.{ $break }vuelve a ejecutarlo con { $yes } para continuar
    }
}

journal.cli.reprocess.range.saved = { $days_quantity ->
    [one] { $segments_quantity ->
        [one] se guardaron solicitudes from-scratch para { $days } día ({ $segments } segmento){ $break }el progreso se ve en health; el trabajo de hoy va primero
        [many] se guardaron solicitudes from-scratch para { $days } día ({ $segments } segmentos){ $break }el progreso se ve en health; el trabajo de hoy va primero
       *[other] se guardaron solicitudes from-scratch para { $days } día ({ $segments } segmentos){ $break }el progreso se ve en health; el trabajo de hoy va primero
    }
    [many] { $segments_quantity ->
        [one] se guardaron solicitudes from-scratch para { $days } días ({ $segments } segmento){ $break }el progreso se ve en health; el trabajo de hoy va primero
        [many] se guardaron solicitudes from-scratch para { $days } días ({ $segments } segmentos){ $break }el progreso se ve en health; el trabajo de hoy va primero
       *[other] se guardaron solicitudes from-scratch para { $days } días ({ $segments } segmentos){ $break }el progreso se ve en health; el trabajo de hoy va primero
    }
   *[other] { $segments_quantity ->
        [one] se guardaron solicitudes from-scratch para { $days } días ({ $segments } segmento){ $break }el progreso se ve en health; el trabajo de hoy va primero
        [many] se guardaron solicitudes from-scratch para { $days } días ({ $segments } segmentos){ $break }el progreso se ve en health; el trabajo de hoy va primero
       *[other] se guardaron solicitudes from-scratch para { $days } días ({ $segments } segmentos){ $break }el progreso se ve en health; el trabajo de hoy va primero
    }
}

journal.cli.reprocess.record.invalid = Registro de unidad diaria mal formado: identidad de unidad diaria no válida

journal.cli.reprocess.record.io = Error de E/S: { $cause }

journal.cli.reprocess.record.mismatch = Registro de unidad diaria mal formado: la identidad no coincide en el registro en { $path }: se esperaba { $expected }, se encontró { $found }

journal.cli.reprocess.retry.accepted = esta salida diaria ya está aceptada

journal.cli.reprocess.retry.disabled = el talento diario ya no está activado

journal.cli.reprocess.retry.invalid.day = día de reintento no válido

journal.cli.reprocess.retry.invalid.unit = unidad de reintento no válida

journal.cli.reprocess.retry.missing = falta la unidad diaria

journal.cli.reprocess.retry.missing.evidence = falta la evidencia conservada

journal.cli.reprocess.retry.no.longer.held = la unidad ya no se conserva

journal.cli.reprocess.retry.not.daily = no es trabajo diario

journal.cli.reprocess.retry.state = esta unidad diaria no se puede reintentar sin resolver su estado actual

journal.cli.reprocess.schedule.lookback = retrospección de la programación diaria fuera de { $range }

journal.cli.reprocess.schema.missing = falta la ruta de la indicación del esquema

journal.cli.reprocess.schema.name = el esquema debe ser un nombre de archivo

journal.cli.reprocess.shape.invalid = forma no válida para { $path }

journal.cli.reprocess.shape.object = el sidecar de forma no es un objeto

journal.cli.reprocess.shape.written = forma de fuente escrita no válida: { $path }

journal.cli.reprocess.source.json.object = { $path } no es un objeto { $encoding }

journal.cli.reprocess.source.line.error = { $path } línea { $line }: { $error }

journal.cli.reprocess.source.line.object = { $path } línea { $line } no es un objeto

journal.cli.reprocess.source.symlink = la fuente diaria es un enlace simbólico: { $path }

journal.cli.reprocess.source.utf8 = ruta de fuente diaria no { $encoding }

journal.cli.reprocess.talent.overrides = no se pudieron leer las anulaciones del talento desde { $path }

journal.cli.reprocess.template.utf8 = nombre de plantilla no { $encoding }

journal.cli.reprocess.through.order = { $through_flag } debe ser el día de inicio o posterior

journal.cli.reprocess.through.required = { $through_flag } requiere { $from_scratch_flag }

journal.cli.reprocess.unit.failed = no se pudo completar la unidad de reprocess: { $error }

journal.cli.reprocess.unit.saved = solicitud de reintento guardada para { $unit } el { $day }

journal.cli.reprocess.unreadable = { $day }  (no se puede leer: { $error })

journal.cli.reprocess.usage.conflict = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: error: argumento { $option }: no se permite con el argumento { $previous }

journal.cli.reprocess.usage.missing.day = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: error: se requieren los siguientes argumentos: { $positional }

journal.cli.reprocess.usage.missing.value = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: error: argumento { $option }: se esperaba un argumento

journal.cli.reprocess.usage.requires = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: error: argumento { $option }: requiere { $required }

journal.cli.reprocess.usage.unknown = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: error: argumentos no reconocidos: { $arguments }

journal.cli.reprocess.waiting = solicitud guardada; inicia el servicio en segundo plano con { $start_command } para procesarla

journal.cli.retention.proposal.eligible = elegibilidad de la política: Eligible { $open_brace } anchor: { $anchor }, age_days: { $age_days }, period: Days({ $period_days }) { $close_brace }

journal.cli.retention.proposal.forever = elegibilidad de la política: KeptForever

journal.cli.retention.proposal.no.anchor = elegibilidad de la política: AnchorMissing { $open_brace } anchor: { $anchor } { $close_brace }

journal.cli.retention.proposal.unrecorded = no se pudo completar el retiro antes de que se registrara una propuesta

journal.cli.retention.proposal.young = elegibilidad de la política: TooYoung { $open_brace } anchor: { $anchor }, age_days: { $age_days }, period: Days({ $period_days }) { $close_brace }

journal.cli.retention.recovery.activity.binding = no se pudo revisar tu diario, así que esta eliminación se detuvo

journal.cli.retention.recovery.activity.busy = tu diario está ocupado con otro trabajo

journal.cli.retention.recovery.activity.hold = otra operación del diario necesita atención antes de que este retiro pueda continuar

journal.cli.retention.recovery.activity.reserved = esta ubicación está reservada para una operación del diario y no se puede quitar

journal.cli.retention.recovery.path.directory = hay una carpeta donde se esperaba una grabación

journal.cli.retention.recovery.path.invalid = la ubicación no nombra un archivo dentro de tu diario

journal.cli.retention.recovery.path.outside = esto no está dentro de tu diario, así que se dejó como estaba

journal.cli.retention.recovery.path.permission = no se pudo leer o quitar el archivo, lo que suele indicar un problema de permisos

journal.cli.retention.recovery.path.unavailable = no se pudo quitar el archivo; el diario quedó sin cambios

journal.cli.retention.recovery.record.prepare = no se pudo preparar el registro de este retiro

journal.cli.retention.recovery.record.write = no se pudo escribir el registro de este retiro

journal.cli.retention.recovery.segment.busy = otro proceso está trabajando en este segmento

journal.cli.retention.recovery.segment.gone = este segmento ya se quitó

journal.cli.retention.recovery.segment.live.occupied = se escribió algo nuevo donde estaba este segmento, así que quedó apartado y no se sobrescribió

journal.cli.retention.recovery.segment.missing = no hay ese segmento en tu diario

journal.cli.retention.recovery.segment.staged = este segmento está a medias del retiro y apartado; no se lista con tus otros segmentos hasta que termine

journal.cli.retention.recovery.segment.unconfirmed = no se pudo confirmar que ya no está, así que no se está informando como quitado

journal.cli.retention.recovery.segment.unfinished = un retiro anterior de este segmento no terminó; hay que revisarlo antes de reintentarlo

journal.cli.retention.removal.uncertain = no se pudo completar ni confirmar la eliminación. hace falta una recuperación.

journal.cli.schedule.cell.disabled = desactivado

journal.cli.schedule.cell.invalid = no válido

journal.cli.schedule.cell.midnight = medianoche

journal.cli.schedule.cell.never = nunca

journal.cli.schedule.cell.now = ahora

journal.cli.schedule.config = Configuración: { $path }

journal.cli.schedule.empty = No hay programaciones configuradas.{ $line_break }{ $line_break }Agrega programaciones en: { $path }

journal.cli.schedule.error.cmd.invalid = la programación '{ $name }' tiene un cmd no válido

journal.cli.schedule.error.config.malformed = la configuración de programaciones en { $path } está mal formada

journal.cli.schedule.error.config.not.object = la configuración de programaciones en { $path } debe ser un objeto JSON

journal.cli.schedule.error.config.unreadable = no se puede leer la configuración de programaciones en { $path }

journal.cli.schedule.error.entry.invalid = la programación '{ $name }' no es una entrada válida

journal.cli.schedule.error.state.malformed = el estado del programador en { $path } está mal formado

journal.cli.schedule.error.state.not.object = el estado del programador en { $path } debe ser un objeto JSON

journal.cli.schedule.error.state.unreadable = no se puede leer el estado del programador en { $path }

journal.cli.schedule.header.cmd = COMANDO

journal.cli.schedule.header.every = CADA

journal.cli.schedule.header.last.run = ÚLTIMA EJECUCIÓN

journal.cli.schedule.header.name = NOMBRE

journal.cli.schedule.header.next.due = PRÓXIMA VEZ

journal.cli.schedule.tag.disabled = [desactivado]

journal.cli.schedule.weekly.friday = viernes { $time }

journal.cli.schedule.weekly.monday = lunes { $time }

journal.cli.schedule.weekly.saturday = sábado { $time }

journal.cli.schedule.weekly.sunday = domingo { $time }

journal.cli.schedule.weekly.thursday = jueves { $time }

journal.cli.schedule.weekly.tuesday = martes { $time }

journal.cli.schedule.weekly.wednesday = miércoles { $time }

journal.cli.segment.argument.missing = uso: { $syntax }{ $line_break }{ $command }: error: se requieren los siguientes argumentos: { $input }

journal.cli.segment.argument.unknown = uso: { $syntax }{ $line_break }{ $command }: error: argumentos no reconocidos: { $arguments }

journal.cli.segment.check.backward = cadena hacia atrás

journal.cli.segment.check.chain.no.day = { $prev_segment_key } definido sin { $prev_day_key }

journal.cli.segment.check.chain.no.successor = no se encontró el segmento siguiente, no es el final del flujo

journal.cli.segment.check.chain.previous = se encontró el segmento anterior

journal.cli.segment.check.chain.start = no hay segmento anterior

journal.cli.segment.check.chain.tail = final del flujo

journal.cli.segment.check.content.empty = no hay { $audio_file }, { $screen_file } ni { $browser_files }

journal.cli.segment.check.content.present = archivos de contenido presentes

journal.cli.segment.check.directory.absent = falta el directorio del segmento

journal.cli.segment.check.directory.exists = el directorio existe

journal.cli.segment.check.directory.missing = falta el directorio

journal.cli.segment.check.forward = cadena hacia adelante

journal.cli.segment.check.index = presencia del índice

journal.cli.segment.check.index.absent = el índice del diario no está disponible

journal.cli.segment.check.index.error = error del índice del diario: { $error } (ejecuta: { $index_command })

journal.cli.segment.check.index.missing = segmento sin indexar

journal.cli.segment.check.index.present = segmento indexado

journal.cli.segment.check.marker.exists = { $stream_file } existe

journal.cli.segment.check.marker.invalid = { $stream_file } { $json_format } no válido

journal.cli.segment.check.marker.missing = falta { $stream_file }

journal.cli.segment.check.marker.no.stream = { $stream_file } sin el campo stream

journal.cli.segment.check.marker.valid = { $stream_file } válido

journal.cli.segment.check.next = segmento siguiente { $path }

journal.cli.segment.check.previous.missing = falta el segmento anterior { $path }

journal.cli.segment.day.empty = No se encontraron segmentos para { $day }

journal.cli.segment.guard.activity = otra operación del diario necesita atención antes de que este cambio pueda continuar

journal.cli.segment.guard.binding = no se pudo revisar el diario, así que este cambio está en pausa

journal.cli.segment.guard.reserved = esta ubicación está reservada para una operación del diario y no se puede cambiar

journal.cli.segment.help.inspect = uso: { $syntax }{ $break }{ $break }argumentos posicionales:{ $break }{ $option_3 }Ruta del segmento: { $example_path } (p. ej.{ $break }{ $syntax_line_4 }{ $break }{ $break }opciones:{ $break }{ $option_7 }muestra este mensaje de ayuda y sale{ $break }{ $option_8 }Salida como { $json_format }

journal.cli.segment.help.list = uso: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $syntax_line_2 }{ $break }{ $break }argumentos posicionales:{ $break }{ $option_5 }Día en formato { $day_format }{ $break }{ $break }opciones:{ $break }{ $option_8 }muestra este mensaje de ayuda y sale{ $break }{ $option_9 }Filtra a un flujo específico{ $break }{ $option_10 }Salida como { $json_format }

journal.cli.segment.help.move = uso: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $syntax_line_2 }{ $break }{ $syntax_line_3 }{ $break }{ $break }argumentos posicionales:{ $break }{ $option_6 }Ruta del segmento: { $example_path } (p. ej.{ $break }{ $syntax_line_7 }{ $break }{ $break }opciones:{ $break }{ $option_10 }muestra este mensaje de ayuda y sale{ $break }{ $option_11 }Día de destino ({ $day_format }){ $break }{ $option_12 }Hora nueva ({ $time_format }), conservando la duración original{ $break }{ $option_13 }Muestra el plan sin hacer cambios{ $break }{ $option_14 }Muestra el progreso detallado

journal.cli.segment.help.top = uso: { $syntax }{ $break }{ $break }Inspecciona y administra los segmentos del diario{ $break }{ $break }argumentos posicionales:{ $break }{ $syntax_line_5 }{ $break }{ $option_6 }Lista los segmentos de un día{ $break }{ $option_7 }Muestra los metadatos del segmento{ $break }{ $option_8 }Verifica la integridad del segmento{ $break }{ $option_9 }Mueve el segmento a otro día u hora{ $break }{ $break }opciones:{ $break }{ $option_12 }muestra este mensaje de ayuda y sale{ $break }{ $option_13 }Activa la salida detallada{ $break }{ $option_14 }Activa el registro de depuración

journal.cli.segment.help.verify = uso: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $break }argumentos posicionales:{ $break }{ $option_4 }Ruta del segmento: { $example_path }{ $break }{ $break }opciones:{ $break }{ $option_7 }muestra este mensaje de ayuda y sale{ $break }{ $option_8 }Verifica todos los segmentos de un día{ $break }{ $option_9 }Salida como { $json_format }

journal.cli.segment.identity.default = lista de segmentos rechazada: el directorio de flujo llamado "{ $default_stream }" no se puede escribir como identidad de registro: { $path }

journal.cli.segment.identity.duplicate = lista de segmentos rechazada: varios segmentos comparten el flujo { $stream } y la clave { $key }

journal.cli.segment.identity.encoding = lista de segmentos rechazada: la ruta del segmento no se puede representar en { $encoding }: { $path }

journal.cli.segment.identity.refused = lista de segmentos rechazada: { $detail }

journal.cli.segment.inspect.chain = Cadena:

journal.cli.segment.inspect.events = { $quantity ->
    [one] Eventos: { $entries } entrada
    [many] Eventos: { $entries } entradas
   *[other] Eventos: { $entries } entradas
}

journal.cli.segment.inspect.events.tracts = { $quantity ->
    [one] Eventos: { $entries } entrada ({ $tracts })
    [many] Eventos: { $entries } entradas ({ $tracts })
   *[other] Eventos: { $entries } entradas ({ $tracts })
}

journal.cli.segment.inspect.files = Archivos ({ $count }):

journal.cli.segment.inspect.index.error = Índice: error ({ $error }) - ejecuta: { $index_command }

journal.cli.segment.inspect.index.missing = Índice: sin indexar

journal.cli.segment.inspect.index.unavailable = Índice: no disponible

journal.cli.segment.inspect.indexed = { $quantity ->
    [one] Índice: indexado ({ $chunks } fragmento)
    [many] Índice: indexado ({ $chunks } fragmentos)
   *[other] Índice: indexado ({ $chunks } fragmentos)
}

journal.cli.segment.inspect.next = siguiente: { $next }

journal.cli.segment.inspect.next.none = siguiente: (ninguno)

journal.cli.segment.inspect.next.tail = siguiente: (final del flujo)

journal.cli.segment.inspect.path = Segmento: { $path }

journal.cli.segment.inspect.prev = previo: { $previous }

journal.cli.segment.inspect.prev.missing = previo: { $path } [AUSENTE]

journal.cli.segment.inspect.prev.none = previo: (ninguno)

journal.cli.segment.inspect.size = Tamaño: { $size }

journal.cli.segment.inspect.stream = Flujo:  { $stream }

journal.cli.segment.inspect.stream.seq = Flujo:  { $stream } (sec { $sequence })

journal.cli.segment.inspect.talents = Talentos ({ $count }):

journal.cli.segment.inspect.time = Hora:    { $range } ({ $seconds } s)

journal.cli.segment.list.heading.dur = DUR

journal.cli.segment.list.heading.files = ARCHIVOS

journal.cli.segment.list.heading.segment = SEGMENTO

journal.cli.segment.list.heading.size = TAMAÑO

journal.cli.segment.list.heading.stream = FLUJO

journal.cli.segment.list.heading.talents = TALENTOS

journal.cli.segment.list.heading.time = HORA

journal.cli.segment.missing = Segmento no encontrado: { $path }

journal.cli.segment.move.day.format = Formato de { $to_day_flag } no válido: { $to_day } (se esperaba { $date_format })

journal.cli.segment.move.directory.created = directorio creado: { $path }

journal.cli.segment.move.directory.exists = El destino { $destination } ya existe; no se hicieron cambios

journal.cli.segment.move.directory.failed = paso 1, no se pudo mover el directorio: { $error }; la fuente y el índice siguen siendo la autoridad

journal.cli.segment.move.directory.moved = directorio movido: { $source } -> { $destination }

journal.cli.segment.move.dry = [simulación] No se hicieron cambios

journal.cli.segment.move.events.none = no hay { $event_file } para reescribir

journal.cli.segment.move.events.rewritten = { $quantity ->
    [one] se reescribió { $count } línea de { $event_file } (día: { $source_day }->{ $destination_day }, segmento: { $source }->{ $destination })
    [many] se reescribieron { $count } líneas de { $event_file } (día: { $source_day }->{ $destination_day }, segmento: { $source }->{ $destination })
   *[other] se reescribieron { $count } líneas de { $event_file } (día: { $source_day }->{ $destination_day }, segmento: { $source }->{ $destination })
}

journal.cli.segment.move.exists = El segmento { $segment } ya existe en { $day }/{ $stream }. Usa { $to_time_flag } para indicar otra hora.

journal.cli.segment.move.fail.ambiguous = { $quantity ->
    [one] paso 3, parche del sucesor inconsistente tras mover: cadena ambigua: { $count } sucesor sin parche
    [many] paso 3, parche del sucesor inconsistente tras mover: cadena ambigua: { $count } sucesores sin parche
   *[other] paso 3, parche del sucesor inconsistente tras mover: cadena ambigua: { $count } sucesores sin parche
}

journal.cli.segment.move.fail.events = paso 2, reescritura de eventos inconsistente tras mover: { $detail }

journal.cli.segment.move.fail.health.marker = paso 6, el toque del marcador de estado quedó inconsistente tras mover: { $detail }

journal.cli.segment.move.fail.search = paso 5, actualización de búsqueda inconsistente tras mover: no se pudo poner en cola: { $error }

journal.cli.segment.move.fail.successor = paso 3, parche del sucesor inconsistente tras mover: { $detail }

journal.cli.segment.move.fail.tail.locked = paso 4, reparación del final del flujo inconsistente tras mover: no se pudo bloquear el registro del flujo

journal.cli.segment.move.fail.tail.malformed = paso 4, reparación del final del flujo inconsistente tras mover: registro del flujo mal formado

journal.cli.segment.move.fail.tail.no.record = paso 4, reparación del final del flujo inconsistente tras mover: no hay registro del flujo; no se reescribió la identidad

journal.cli.segment.move.fail.tail.write = paso 4, reparación del final del flujo inconsistente tras mover: no se pudo completar la publicación atómica

journal.cli.segment.move.markers.touched = se tocaron los marcadores de estado: { $days }

journal.cli.segment.move.no.duration = La clave del segmento de origen no tiene duración

journal.cli.segment.move.no.marker = No hay { $stream_file } en el segmento de origen

journal.cli.segment.move.no.slot = No hay ranura de segmento libre cerca de { $destination_segment } el { $to_day }

journal.cli.segment.move.plan = Mover: { $source } -> { $destination }

journal.cli.segment.move.plan.ambiguous = { $quantity ->
    [one] sucesor por parchear: (ambiguo: { $count } sucesor)
    [many] sucesor por parchear: (ambiguo: { $count } sucesores)
   *[other] sucesor por parchear: (ambiguo: { $count } sucesores)
}

journal.cli.segment.move.plan.events = { $event_file } líneas: { $count }

journal.cli.segment.move.plan.index = fragmentos del índice: { $chunks }

journal.cli.segment.move.plan.index.error = error al leer el índice: { $error } (se intentará eliminar+reindexar; ejecuta: { $index_command })

journal.cli.segment.move.plan.markers = marcadores de estado: { $source_day }, { $destination_day }

journal.cli.segment.move.plan.successor = sucesor por parchear: { $path }

journal.cli.segment.move.plan.tail = sucesor por parchear: (ninguno - final del flujo)

journal.cli.segment.move.running = Ejecutando el movimiento...

journal.cli.segment.move.same = El origen y el destino son el mismo

journal.cli.segment.move.search.queued = actualizaciones de búsqueda en cola

journal.cli.segment.move.stream.mismatch = El flujo no coincide: la ruta dice '{ $path_stream }', pero { $stream_file } dice '{ $marker_stream }'

journal.cli.segment.move.stream.rebuilt = se reconstruyó el estado del flujo: { $stream }

journal.cli.segment.move.successor.none = no hay sucesor por parchear (final del flujo)

journal.cli.segment.move.successor.patched = sucesor parcheado { $path }

journal.cli.segment.move.talents = { $command } volverá a ejecutar los talentos diarios en los días tocados

journal.cli.segment.move.time.format = Formato de { $to_time_flag } no válido: { $time } (se esperaba { $time_format })

journal.cli.segment.path.syntax = La ruta del segmento debe ser { $path_form } (p. ej. { $example })

journal.cli.segment.relocation.deleted = el segmento fue eliminado por su propietario

journal.cli.segment.relocation.location = la ubicación del segmento no coincide con el movimiento planeado

journal.cli.segment.relocation.marker = no se pudo leer el marcador

journal.cli.segment.status.pass = PASS

journal.cli.segment.verify.day.result = { $quantity ->
    [one] Resumen: { $passed }/{ $total } revisión superada
    [many] Resumen: { $passed }/{ $total } revisiones superadas
   *[other] Resumen: { $passed }/{ $total } revisiones superadas
}

journal.cli.segment.verify.input = { $verb } requiere una ruta de segmento o { $day_flag }

journal.cli.segment.verify.result = { $quantity ->
    [one] { $passed }/{ $total } revisión superada
    [many] { $passed }/{ $total } revisiones superadas
   *[other] { $passed }/{ $total } revisiones superadas
}

journal.cli.share.activities = no se pudieron leer las actividades de { $facet } del { $day }: { $detail }

journal.cli.share.activity.missing = no hay actividad '{ $activity }' en { $facet } el { $day }

journal.cli.share.activity.unmatched = la actividad '{ $activity }' no coincide con { $person }

journal.cli.share.facets = no se pudieron listar las facetas: { $detail }

journal.cli.share.failure = { $command }: { $detail }

journal.cli.share.help.match = uso: { $syntax }{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $person_option }{ $gap_3 }id de entidad, o un nombre o alias único de una persona{ $line_break }{ $indent_2 }{ $day_option }{ $gap_4 }el día que se lee{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_5 }solo esta faceta (predeterminado: todas las facetas)

journal.cli.share.help.package = uso: { $syntax }{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $person_option }{ $gap_3 }id de entidad, o un nombre o alias único de una persona{ $line_break }{ $indent_2 }{ $day_option }{ $gap_4 }el día de la actividad{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_5 }la faceta de la actividad{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_5 }el id de la actividad

journal.cli.share.help.root = uso: { $syntax }{ $line_break }{ $line_break }Encuentra actividades compartidas con una persona vinculada y previsualiza lo que se compartiría.{ $line_break }Ambos subcomandos solo leen el diario.{ $line_break }{ $line_break }comandos:{ $line_break }{ $match_command }Lista las actividades del día que coinciden con una persona, una línea { $json } por cada una{ $line_break }{ $package_command }Imprime el paquete de resumen acotado de una actividad que coincide

journal.cli.share.parse.argument = { $command }: error: argumento no reconocido '{ $argument }'

journal.cli.share.parse.duplicate = { $command }: error: { $flag } se indicó más de una vez

journal.cli.share.parse.encoding = { $command }: error: los argumentos deben ser { $encoding } válido

journal.cli.share.parse.missing = { $command }: error: se requiere un subcomando

journal.cli.share.parse.required = { $command }: error: { $option } es obligatorio

journal.cli.share.parse.unknown = { $command }: error: subcomando desconocido '{ $argument }'

journal.cli.share.parse.value = { $command }: error: { $flag } necesita un valor

journal.cli.share.provider.activity.unavailable = resumen de actividad no disponible: { $detail }

journal.cli.share.provider.child.cleanup = { $primary } (limpieza: { $cleanup })

journal.cli.share.provider.child.exit = salida { $code }

journal.cli.share.provider.child.exit.signal = salida { $code } señal { $signal }

journal.cli.share.provider.child.invalid = respuesta del proceso hijo no válida ({ $status }): { $detail }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.process = falló la E/S del proceso: { $primary }; { $status_field }{ $status }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.process.cleanup = falló la E/S del proceso: { $primary }; { $status_field }{ $status }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }; { $cleanup_field }{ $cleanup }

journal.cli.share.provider.child.protocol = protocolo { $reason } ({ $status }): { $detail }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.signal = señal { $signal }

journal.cli.share.provider.child.status.unknown = estado desconocido

journal.cli.share.provider.child.truncated = { $bytes } [truncado]

journal.cli.share.provider.child.unavailable = no disponible

journal.cli.share.provider.child.unexpected = proceso hijo inesperado ({ $status }); { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.entities.read = no se pudieron leer las entidades: { $detail }

journal.cli.share.provider.entity.invalid = no se pudo cargar la entidad del diario { $entity }: { $detail }

journal.cli.share.provider.matching = coincidiendo { $facet }/{ $day }/{ $activity } para { $entity }

journal.cli.share.provider.model.unconfigured = Todavía no hay un modelo elegido. Elige uno en Razonamiento.

journal.cli.share.provider.person.absent = ninguna persona de este diario se llama '{ $query }'

journal.cli.share.provider.person.ambiguous = '{ $query }' es el nombre de más de una persona ({ $people }); usa un id de entidad

journal.cli.share.provider.presence.unavailable = revisión de presencia no disponible: { $detail }

journal.cli.share.provider.segment.unavailable = resumen de segmento no disponible: { $detail }

journal.cli.share.provider.talent.invalid = el talento '{ $talent }' devolvió { $json } no válido: { $error }

journal.cli.share.provider.talent.prepare = talento '{ $talent }': { $detail }

journal.cli.share.provider.talent.timeout.numeric = la configuración '{ $talent }' declara un { $timeout_s_field } que no es numérico

journal.cli.share.provider.talent.timeout.range = la configuración '{ $talent }' declara { $timeout_s_field } fuera del rango admitido

journal.cli.share.provider.talent.tokens = el talento '{ $talent }' no declara { $max_output_tokens_field }

journal.cli.share.provider.talent.type = tipo de talento no admitido: { $kind }

journal.cli.share.provider.talent.unfinished = el talento '{ $talent }' no terminó: { $outcome }

journal.cli.share.startup.failed = { $command } falló: { $detail }

journal.cli.share.startup.runtime.current = no se pudo resolver el ejecutable del proceso: { $error }

journal.cli.share.startup.runtime.parent = el ejecutable del proceso no tiene directorio padre: { $path }

journal.cli.share.startup.runtime.root = no se pudo resolver la raíz de la instalación desde el directorio del ejecutable: { $path }

journal.cli.share.startup.sibling.missing = { $command } falló: falta el ejecutable hermano { $path }

journal.cli.share.startup.sibling.parent = { $command } falló: el ejecutable actual no tiene directorio padre

journal.cli.share.transport.child.inspect = inspeccionar el proceso hijo antes de terminarlo: { $detail }

journal.cli.share.transport.child.reap = recolectar el proceso hijo: { $detail }

journal.cli.share.transport.child.retry = reintentar recolectar el proceso hijo: { $detail }

journal.cli.share.transport.child.terminate = terminar el proceso hijo: { $detail }

journal.cli.share.transport.collector = recolector de { $pipe }: { $detail }

journal.cli.share.transport.collector.panic = recolector de { $pipe }: el hilo entró en pánico

journal.cli.share.transport.field.array = { $field } debe ser un arreglo o null

journal.cli.share.transport.field.boolean = { $field } debe ser un booleano

journal.cli.share.transport.field.optional = { $field } debe ser una cadena o null

journal.cli.share.transport.field.string = { $field } debe ser una cadena

journal.cli.share.transport.field.strings = { $field } debe contener solo cadenas

journal.cli.share.transport.object = el registro debe ser un objeto { $json }

journal.cli.share.transport.outcome = no se admite el resultado de la respuesta

journal.cli.share.transport.pipe.missing = el enlace { $pipe } no está disponible

journal.cli.share.transport.request.empty = { $contents } debe incluir contenido

journal.cli.share.transport.request.overflow = { $overflow } exige que la primera parte del contenido sea texto

journal.cli.share.transport.schema = no se admite el esquema del registro

journal.cli.share.transport.stdout.limit = { $stdout } se truncó en { $limit } bytes

journal.cli.share.transport.usage = { $usage } es obligatorio

journal.cli.share.transport.worker.channel = canal de finalización del proceso de la tubería: { $detail }

journal.cli.share.transport.worker.join = unión del proceso de { $pipe }: el hilo entró en pánico

journal.cli.share.transport.worker.start = iniciar el proceso de { $pipe }: { $detail }

journal.cli.share.transport.writer = escritor de { $stdin }: { $detail }

journal.cli.share.transport.writer.panic = escritor de { $stdin }: el hilo entró en pánico

journal.cli.share.usage.match = uso: { $syntax }

journal.cli.share.usage.package = uso: { $syntax }

journal.cli.share.usage.root = uso: { $syntax }

journal.cli.social.failed = no se pudo terminar: { $detail }

journal.cli.social.follow.done = siguiendo el espacio que { $sharer } compartió contigo.

journal.cli.social.receive.added = { $activity_quantity ->
    [one] { $sharer } compartió { $activities } actividad nueva contigo.
    [many] { $sharer } compartió { $activities } actividades nuevas contigo.
   *[other] { $sharer } compartió { $activities } actividades nuevas contigo.
}

journal.cli.social.receive.failed = no se pudo leer el espacio que { $sharer } compartió contigo: { $detail }

journal.cli.social.receive.none = nada nuevo de { $sharer }.

journal.cli.social.receive.nothing = todavía no sigues ningún espacio.

journal.cli.social.receive.removed = { $activity_quantity ->
    [one] { $sharer } dejó de compartir { $activities } actividad, así que ya no está en tu diario.
    [many] { $sharer } dejó de compartir { $activities } actividades, así que ya no están en tu diario.
   *[other] { $sharer } dejó de compartir { $activities } actividades, así que ya no están en tu diario.
}

journal.cli.social.receive.updated = { $activity_quantity ->
    [one] { $sharer } actualizó { $activities } actividad que compartió contigo.
    [many] { $sharer } actualizó { $activities } actividades que compartió contigo.
   *[other] { $sharer } actualizó { $activities } actividades que compartió contigo.
}

journal.cli.social.resolve.done = { $account } es { $did }, con alojamiento en { $host }.

journal.cli.social.share.done = compartiste "{ $title }". envía esta dirección del espacio a las personas con las que lo compartiste, para que sus diarios puedan seguirlo:{ $line_break }{ $space }

journal.cli.social.signed.out = no iniciaste sesión. empieza con { $sign_in }

journal.cli.social.signin.declined = se rechazó el inicio de sesión, así que no cambió nada.

journal.cli.social.signin.done = iniciaste sesión como { $account }.

journal.cli.social.signin.expired = el inicio de sesión no se terminó a tiempo. ejecútalo otra vez para empezar de cero.

journal.cli.social.signin.open = para iniciar sesión, abre esta dirección en tu navegador:{ $line_break }{ $address }

journal.cli.social.status.account = iniciaste sesión como { $account } en { $host }.

journal.cli.social.status.memories = { $memory_quantity ->
    [one] guardando { $memories } actividad compartida contigo.
    [many] guardando { $memories } actividades compartidas contigo.
   *[other] guardando { $memories } actividades compartidas contigo.
}

journal.cli.social.status.shares = { $share_quantity ->
    [one] compartiendo { $shares } actividad.
    [many] compartiendo { $shares } actividades.
   *[other] compartiendo { $shares } actividades.
}

journal.cli.social.status.spaces = { $space_quantity ->
    [one] siguiendo { $spaces } espacio.
    [many] siguiendo { $spaces } espacios.
   *[other] siguiendo { $spaces } espacios.
}

journal.cli.social.sync.current = todas las actividades compartidas están al día.

journal.cli.social.sync.removed = { $activity_quantity ->
    [one] se quitó { $activities } actividad compartida que ya no está en tu diario.
    [many] se quitaron { $activities } actividades compartidas que ya no están en tu diario.
   *[other] se quitaron { $activities } actividades compartidas que ya no están en tu diario.
}

journal.cli.social.sync.updated = { $activity_quantity ->
    [one] se actualizó { $activities } actividad compartida.
    [many] se actualizaron { $activities } actividades compartidas.
   *[other] se actualizaron { $activities } actividades compartidas.
}

journal.cli.social.unshare.done = se dejó de compartir esa actividad, y ya no está en ningún espacio en el que la compartiste.

journal.cli.social.unshare.unknown = esa actividad no está compartida.

journal.cli.social.usage = uso: { $syntax }{ $line_break }{ $line_break }comparte una actividad de tu diario con las personas que elijas, a través de un espacio en tu propia cuenta de atproto, y sigue los espacios que otras personas comparten contigo. tu diario conserva lo que comparten aparte de tus propios recuerdos. solo en compilaciones de desarrollo.

journal.cli.speakers.backfill.error.flags.mismatch = los indicadores inmutables del relleno no coinciden para la operación { $operation_id }: se esperaba commit={ $expected_commit } reattribute={ $expected_reattribute } accumulation={ $expected_accumulation }, se obtuvo commit={ $got_commit } reattribute={ $got_reattribute } accumulation={ $got_accumulation }

journal.cli.speakers.backfill.error.invalid.flags = indicadores de relleno no válidos: { $detail }

journal.cli.speakers.backfill.error.journal.root = no se pudo admitir la raíz del diario: { $error }

journal.cli.speakers.backfill.error.ledger = falló la operación del libro mayor: { $error }

journal.cli.speakers.backfill.error.lock = no se pudo obtener el bloqueo de ejecución del relleno: { $error }

journal.cli.speakers.backfill.error.not.found = no se encontró la operación de relleno: { $id }

journal.cli.speakers.backfill.error.scan = no se pudo escanear el relleno: { $error }

journal.cli.speakers.owner.voice.error.identity.invalid = la identidad de propietario configurada no está admitida

journal.cli.speakers.owner.voice.error.no.candidate = No hay un candidato disponible

journal.cli.speakers.owner.voice.error.no.saved.voice = No hay una voz de propietario guardada

journal.cli.speakers.owner.voice.error.stale.candidate = La versión del candidato no coincide, o la generación está desactualizada

journal.cli.speakers.pool.error.centroid.width = el ancho del centroide del conjunto de candidatos no coincide

journal.cli.speakers.pool.error.lock = no se pudo bloquear el conjunto de candidatos: { $error }

journal.cli.speakers.pool.error.read = no se pudo leer el conjunto de candidatos: { $error }

journal.cli.speakers.pool.error.write = no se pudo escribir el conjunto de candidatos: { $error }

journal.cli.speakers.pool.refusal.days.unreadable = se rechazó la puesta al día de la lista de voces: no se pudieron listar los días del diario

journal.cli.speakers.pool.refusal.format.unsupported = se rechazó la puesta al día de la lista de voces: la lista de voces está en un formato al que esta versión no puede agregar

journal.cli.speakers.pool.refusal.lock.timeout = se rechazó la puesta al día de la lista de voces: se agotó el tiempo de bloqueo de la lista de voces

journal.cli.speakers.pool.refusal.lock.timeout.kept = se rechazó la puesta al día de la lista de voces: se agotó el tiempo de bloqueo de la lista de voces; se conserva lo que se agregó

journal.cli.speakers.pool.refusal.pool.unreadable = se rechazó la puesta al día de la lista de voces: no se pudo leer la lista de voces

journal.cli.speakers.pool.refusal.pool.write.failed = se rechazó la puesta al día de la lista de voces: no se pudo escribir la lista de voces

journal.cli.speakers.pool.refusal.pool.write.failed.kept = se rechazó la puesta al día de la lista de voces: no se pudo escribir la lista de voces; se conserva lo que se agregó

journal.cli.speakers.pool.refusal.progress.invalid = se rechazó la puesta al día de la lista de voces: el archivo de progreso de la puesta al día no es válido; quitar awareness/speaker_candidates_backfill.json de tu diario reinicia la puesta al día de forma segura

journal.cli.speakers.pool.refusal.progress.unreadable = se rechazó la puesta al día de la lista de voces: no se pudo leer el archivo de progreso de la puesta al día; revisa que se pueda leer la carpeta awareness de tu diario

journal.cli.speakers.pool.refusal.progress.write.failed = se rechazó la puesta al día de la lista de voces: no se pudo escribir el archivo de progreso de la puesta al día

journal.cli.speakers.pool.refusal.progress.write.failed.kept = se rechazó la puesta al día de la lista de voces: no se pudo escribir el archivo de progreso de la puesta al día; se conserva lo que se agregó

journal.cli.stats.arguments = uso: { $syntax }{ $line_break }{ $command }: error: argumentos no reconocidos: { $argument }

journal.cli.stats.cause.facet = error del almacén de facetas: { $detail }

journal.cli.stats.cause.invalid.day = día no válido { $day }; se esperaba una fecha de calendario { $date_format }

journal.cli.stats.cause.io = no se puede acceder a { $path }: { $detail }

journal.cli.stats.cause.path = error de ruta del diario: { $detail }

journal.cli.stats.cause.read = error de lectura del diario: { $detail }

journal.cli.stats.cause.schema = no se pudieron validar las estadísticas: { $schema } es { $actual }, se esperaba { $expected }

journal.cli.stats.cause.timestamp = no se pudieron validar las estadísticas: { $timestamp } debe incluir contenido

journal.cli.stats.cause.validation = no se pudieron validar las estadísticas: { $detail }

journal.cli.stats.cause.write = error de escritura atómica del diario: { $detail }

journal.cli.stats.configuration = Error al cargar la configuración diaria del diario: { $error }

journal.cli.stats.day.cache = No se pudo guardar la caché del día { $day }: { $error }

journal.cli.stats.enumerate = Error al enumerar los días del diario: { $error }

journal.cli.stats.help = uso: { $syntax }{ $line_break }{ $line_break }Examina un diario de { $brand } y genera estadísticas{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_5 }muestra este mensaje de ayuda y sal{ $line_break }{ $indent_2 }{ $no_cache_option }{ $gap_5 }Desactiva la caché por día (fuerza un nuevo examen de todos los días){ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_2 }Activa la salida detallada{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_4 }Activa el registro de depuración

journal.cli.stats.overrides = no se pudieron validar las estadísticas: no se pudieron leer las anulaciones del talento en { $path }

journal.cli.stats.publication.durable = publicado; durabilidad incierta: { $source }

journal.cli.stats.publication.observed = publicado; observación incierta: { $source }

journal.cli.stats.roots = journal-stats falló: { $error }

journal.cli.stats.saved = Estadísticas guardadas en { $path }

journal.cli.stats.tokens.directory = No se pudo leer el directorio de tokens: { $path }

journal.cli.stats.tokens.json = Token { $json_format } no válido en { $path }: { $error }

journal.cli.stats.tokens.load = No se pudo cargar la caché de tokens de { $path }: { $error }

journal.cli.stats.tokens.read = No se pudo leer el token de { $path }: { $error }

journal.cli.stats.tokens.save = No se pudo guardar la caché de tokens de { $path }: { $error }

journal.cli.stats.validate = Error al validar { $filename }: { $error }

journal.cli.stats.write = Error al escribir { $filename }: { $error }

journal.cli.stats.written = { $days ->
    [one] Se escribieron estadísticas de { $days } día en { $path }
    [many] Se escribieron estadísticas de { $days } días en { $path }
   *[other] Se escribieron estadísticas de { $days } días en { $path }
}

journal.cli.streams.arguments = uso: { $syntax }{ $break }{ $command }: error: argumentos no reconocidos: { $arguments }

journal.cli.streams.empty = No se encontraron flujos

journal.cli.streams.header.day = Último día

journal.cli.streams.header.name = Nombre

journal.cli.streams.header.segment = Último segmento

journal.cli.streams.header.sequence = Sec.

journal.cli.streams.header.type = Tipo

journal.cli.streams.help = uso: { $syntax }{ $paragraph }Inspecciona y administra la identidad del flujo{ $paragraph }argumentos posicionales:{ $break }{ $indent }{ $name_input }{ $name_space }Nombre del flujo que se inspecciona (omítelo para listar todos los flujos){ $paragraph }opciones:{ $break }{ $indent }{ $help_input }{ $help_space }muestra este mensaje de ayuda y sal{ $break }{ $indent }{ $rebuild_input }{ $rebuild_space }Reconstruye el estado del flujo a partir de los marcadores por segmento{ $break }{ $indent }{ $verbose_input }{ $verbose_space }Activa la salida detallada{ $break }{ $indent }{ $debug_input }{ $debug_space }Activa el registro de depuración

journal.cli.streams.inspect.malformed = No se pudo leer el flujo { $path }: registro mal formado

journal.cli.streams.inspect.not.found = No se encontró el flujo: { $name }

journal.cli.streams.list.error = No se pudieron listar los flujos: { $error }

journal.cli.streams.marker.sequence = marcador ilegible { $path }: secuencia no válida

journal.cli.streams.marker.unreadable = marcador ilegible { $path }: no se pudo leer el marcador

journal.cli.streams.rebuild.empty = { $segment_quantity ->
    [one] No se encontraron flujos ({ $segments } segmento examinado)
    [many] No se encontraron flujos ({ $segments } segmentos examinados)
   *[other] No se encontraron flujos ({ $segments } segmentos examinados)
}

journal.cli.streams.rebuild.result = { $stream_quantity ->
    [one] Se reconstruyó { $streams } flujo a partir de { $segment_quantity ->
        [one] { $segments } segmento:
        [many] { $segments } segmentos:
       *[other] { $segments } segmentos:
    }
    [many] Se reconstruyeron { $streams } flujos a partir de { $segment_quantity ->
        [one] { $segments } segmento:
        [many] { $segments } segmentos:
       *[other] { $segments } segmentos:
    }
   *[other] Se reconstruyeron { $streams } flujos a partir de { $segment_quantity ->
        [one] { $segments } segmento:
        [many] { $segments } segmentos:
       *[other] { $segments } segmentos:
    }
}

journal.cli.streams.record.malformed = No se pudo leer el registro del flujo { $path }: registro mal formado

journal.cli.streams.repair.ahead = { $stream }: la secuencia del estado del flujo va por delante de los marcadores; sin cambios

journal.cli.streams.repair.locked = { $stream }: no se pudo bloquear { $path }; sin cambios

journal.cli.streams.repair.malformed = { $stream }: registro de flujo mal formado en { $path }; sin cambios

journal.cli.streams.repair.missing = { $stream }: no hay registro de flujo en { $path }; sin cambios

journal.cli.streams.repair.write.failed = { $stream }: no se pudo escribir { $path }: falló la publicación atómica

journal.cli.streams.segments.error = No se pudieron listar los segmentos: { $error }

journal.cli.streams.supervisor = el diario no está en ejecución. inícialo con '{ $command }' y reinténtalo.

journal.cli.talent.compose.facet.missing = no se encontró la faceta '{ $facet }'

journal.cli.talent.compose.prompt.path.has.no.parent = talento { $talent }: la ruta de la indicación no tiene directorio padre

journal.cli.talent.compose.prompt.path.is.missing = talento { $talent }: falta la ruta de la indicación

journal.cli.talent.compose.schema.directory.is.unavailable = talento { $talent }: el directorio del esquema no está disponible: { $error }

journal.cli.talent.compose.schema.file.could.not.be.read = talento { $talent }: no se pudo leer el archivo del esquema: { $error }

journal.cli.talent.compose.schema.file.is.not.a.valid.json.schema = talento { $talent }: el archivo del esquema no es válido como { $schema_format }: { $path }

journal.cli.talent.compose.schema.file.is.not.valid.json = talento { $talent }: el archivo del esquema no es { $json } válido: { $path }

journal.cli.talent.compose.schema.file.not.found.cause = talento { $talent }: no se encontró el archivo del esquema: { $error }

journal.cli.talent.compose.schema.file.not.found.path = talento { $talent }: no se encontró el archivo del esquema: { $path }

journal.cli.talent.compose.schema.must.be.a.string.got = talento { $talent }: el esquema debe ser una cadena, se recibió { $type_name }: { $value }

journal.cli.talent.compose.schema.path.escapes.talent.directory = talento { $talent }: la ruta del esquema escapa del directorio del talento: { $path }

journal.cli.talent.compose.schema.path.must.be.relative = talento { $talent }: la ruta del esquema debe ser relativa: { $path }

journal.cli.talent.compose.schema.path.must.not.contain = talento { $talent }: la ruta del esquema no debe contener '..': { $path }

journal.cli.talent.compose.settings = no se pudo leer la configuración del diario: { $detail }

journal.cli.talent.config.access.retired = La indicación '{ $talent }' establece el campo retirado { $access_tier }

journal.cli.talent.config.activities = La indicación programada por actividad '{ $talent }' debe tener una lista { $activities } que no esté vacía (tipos de actividad con los que coincidir, o { $all_types } para todos los tipos).

journal.cli.talent.config.budget.invalid = La indicación '{ $talent }' tiene un valor no válido de { $max_output_tokens }: { $value }. Se esperaba un entero positivo medido respecto al modelo empaquetado.

journal.cli.talent.config.budget.required = A la indicación '{ $talent }' le falta el campo obligatorio { $max_output_tokens }. Mide su salida más grande en el modelo empaquetado y declara 1.5 veces eso, redondeado hacia arriba a un múltiplo de 256.

journal.cli.talent.config.budget.retired = La indicación '{ $talent }' declara el campo retirado { $thinking_budget }. El razonamiento solo se configura en Razonamiento, para tu propio modelo.

journal.cli.talent.config.cwd.retired = La indicación '{ $talent }' establece el campo retirado { $cwd }

journal.cli.talent.config.frontmatter = no se pudo analizar el frontmatter de { $path }

journal.cli.talent.config.output.required = La indicación '{ $talent }' tiene type={ $generate }, pero le falta el campo { $output } obligatorio.

journal.cli.talent.config.priority = A la indicación programada '{ $talent }' le falta el campo { $priority } obligatorio. Todas las indicaciones con { $schedule } deben declarar una prioridad explícita.

journal.cli.talent.config.read = no se pudo leer { $path }: { $error }

journal.cli.talent.config.stat = no se pudo ejecutar stat en { $path }: { $error }

journal.cli.talent.config.stem = el nombre de archivo del talento no tiene nombre base { $encoding }: { $path }

journal.cli.talent.config.type.invalid = La indicación '{ $talent }' tiene un tipo no válido { $value }. Se esperaba { $generate }.

journal.cli.talent.config.type.required = La indicación '{ $talent }' tiene salida, pero le falta el campo { $type } obligatorio.

journal.cli.talent.daily.accepted.response = falta la respuesta aceptada

journal.cli.talent.daily.accepted.result = falta el resultado diario aceptado

journal.cli.talent.daily.artifact.before = conflicto: el artefacto obligatorio cambió después de preparar la indicación

journal.cli.talent.daily.artifact.changed = conflicto: la salida obligatoria cambió después de la preparación

journal.cli.talent.daily.artifact.snapshot = falta la instantánea congelada del artefacto obligatorio

journal.cli.talent.daily.artifact.snapshot.invalid = instantánea del artefacto congelado no válida: { $error }

journal.cli.talent.daily.attempt.day = falta el día del intento diario

journal.cli.talent.daily.attempt.day.invalid = día del intento diario no válido

journal.cli.talent.daily.attempt.mismatch = no coincide el { $key } diario

journal.cli.talent.daily.attempt.token = falta el token del intento diario

journal.cli.talent.daily.commit.observer = se esperaba el estado de hechos de la entidad

journal.cli.talent.daily.commit.suggest = se esperaba el estado de sugerencias de la entidad

journal.cli.talent.daily.commit.text = se esperaba la salida de texto

journal.cli.talent.daily.facet.changed = conflicto: la faceta dueña cambió después de preparar la indicación

journal.cli.talent.daily.facet.identity = falta la identidad congelada de la faceta

journal.cli.talent.daily.facet.path = ruta de salida de la faceta no válida

journal.cli.talent.daily.maintenance.changed = conflicto: la hora diaria cambió después de preparar la indicación

journal.cli.talent.daily.maintenance.output = salida de mantenimiento no válida: { $error }

journal.cli.talent.daily.maintenance.primary = a la salida de mantenimiento le falta la principal

journal.cli.talent.daily.maintenance.snapshot = falta la instantánea congelada de la hora diaria

journal.cli.talent.daily.observer.changed = conflicto: la observación cambió después de preparar la indicación

journal.cli.talent.daily.observer.conflict = conflicto: { $error }

journal.cli.talent.daily.observer.entity.gone = conflicto: desapareció la entidad de la observación

journal.cli.talent.daily.observer.validation = validación: { $error }

journal.cli.talent.daily.output.outside = la salida diaria está fuera del diario

journal.cli.talent.daily.output.path = ruta de salida diaria no válida

journal.cli.talent.daily.owner.alias.claimed = conflicto: se reclamó el alias de la promoción después de la preparación

journal.cli.talent.daily.owner.alias.target.detached = conflicto: el destino del alias ya no está asociado

journal.cli.talent.daily.owner.identity.blocked.facet = conflicto: el nombre de la promoción está oculto en esta faceta

journal.cli.talent.daily.owner.identity.blocked.hidden = conflicto: la entidad está oculta, así que no se cambiaron sus nombres

journal.cli.talent.daily.owner.identity.blocked.identity = conflicto: la identidad de la promoción está oculta

journal.cli.talent.daily.owner.identity.changed = conflicto: la identidad promovida cambió después de la preparación

journal.cli.talent.daily.owner.identity.deleted = conflicto: la identidad promovida se eliminó después de la preparación

journal.cli.talent.daily.owner.identity.disappeared = conflicto: desapareció la identidad de la promoción

journal.cli.talent.daily.owner.identity.map.group.lost = conflicto: la promoción { $name } coincide con { $directory }, que perdió su grupo del mapa de identidades

journal.cli.talent.daily.owner.identity.merged = conflicto: la identidad promovida se fusionó con otra después de la preparación

journal.cli.talent.daily.owner.identity.moved = conflicto: la identidad promovida se movió después de la preparación

journal.cli.talent.daily.owner.identity.unreadable = conflicto: la promoción { $name } deriva el id de entidad { $slug }, cuya identidad { $directory } no se puede leer

journal.cli.talent.daily.owner.merge.proposals.changed = conflicto: las propuestas de fusión cambiaron después de la preparación

journal.cli.talent.daily.owner.name.matches.multiple = conflicto: la promoción { $name } coincide con { $count } identidades por nombre

journal.cli.talent.daily.owner.no.usable.entity.id = conflicto: la promoción { $name } no tiene un id de entidad utilizable

journal.cli.talent.daily.owner.owning.facet.changed.absent = conflicto: la faceta dueña ya no existe

journal.cli.talent.daily.owner.owning.facet.changed.invalid = conflicto: la faceta dueña no tiene una identidad estable válida

journal.cli.talent.daily.owner.owning.facet.changed.replaced = conflicto: la faceta dueña se reemplazó después de la preparación

journal.cli.talent.daily.owner.principal.grant.refused.exists = conflicto: ya existe un propietario del diario, así que esta persona no queda marcada

journal.cli.talent.daily.owner.principal.grant.refused.name = conflicto: el nombre de propietario configurado ya no coincide con esta persona

journal.cli.talent.daily.owner.principal.settings = no se pudo leer la configuración del diario, así que esta persona no se marcó como propietario

journal.cli.talent.daily.owner.promoted.entity.blocked.hidden = conflicto: la entidad promovida está oculta

journal.cli.talent.daily.owner.promoted.entity.blocked.name = conflicto: la entidad promovida { $name } está oculta

journal.cli.talent.daily.owner.relationship.changed = conflicto: la relación de la promoción cambió después de la preparación

journal.cli.talent.daily.owner.relationship.occupied = conflicto: la promoción { $name } necesita el directorio de relación { $relationship_dir }, que ya está ocupado

journal.cli.talent.daily.packet.absent = el intento diario no tiene un paquete congelado

journal.cli.talent.daily.packet.digest = el resumen del paquete diario no coincide

journal.cli.talent.daily.packet.scope = el alcance del paquete diario no coincide

journal.cli.talent.daily.packet.talent = el talento del paquete diario no coincide

journal.cli.talent.daily.parse.entities = el campo entities debe ser un arreglo

journal.cli.talent.daily.parse.output = no se pudo analizar la salida como JSON: { $error }

journal.cli.talent.daily.publication.plan = la publicación exige el resultado generado que se conservó y el plan preparado exacto

journal.cli.talent.daily.publication.record = falta el registro de publicación

journal.cli.talent.daily.response.malformed = la respuesta diaria que se conservó está mal formada

journal.cli.talent.daily.retry.scope = cambió el alcance del reintento diario

journal.cli.talent.daily.review.aliases = a la promoción de la revisión le falta el campo aliases

journal.cli.talent.daily.review.eligibility = falta la elegibilidad congelada de la revisión

journal.cli.talent.daily.review.hints = faltan las pistas congeladas de la revisión

journal.cli.talent.daily.review.inputs = faltan las entradas congeladas de admisión de la revisión

journal.cli.talent.daily.review.merges = a la salida de la revisión le falta el campo merges

journal.cli.talent.daily.review.name = al candidato congelado le falta el nombre

journal.cli.talent.daily.review.owner = falta el estado congelado del propietario de la revisión

journal.cli.talent.daily.review.promotion = falta el estado congelado del propietario de la promoción

journal.cli.talent.daily.review.promotion.changed = conflicto: el estado del propietario de la promoción cambió después de preparar la indicación

journal.cli.talent.daily.review.promotions = a la salida de la revisión le falta el campo promotions

journal.cli.talent.daily.review.proposals = falta el estado congelado de la propuesta

journal.cli.talent.daily.review.proposals.changed = conflicto: la propuesta de fusión cambió después de preparar la indicación

journal.cli.talent.daily.review.type = al candidato congelado le falta el tipo

journal.cli.talent.daily.type = tipo de talento no admitido: { $kind }

journal.cli.talent.daily.validation.aliases = validación: el candidato promovido de la revisión debe incluir el arreglo aliases

journal.cli.talent.daily.validation.json = validación: JSON { $shape } no válido: { $error }

journal.cli.talent.daily.validation.maintenance.missing = validación: falta el campo primary del mantenimiento

journal.cli.talent.daily.validation.maintenance.time = validación: el campo primary del mantenimiento debe ser HH:MM

journal.cli.talent.daily.validation.merges = validación: el campo merges de la revisión debe ser un arreglo

journal.cli.talent.daily.validation.observer = validación: entities:entity_observer exige un arreglo entities

journal.cli.talent.daily.validation.promotions = validación: el campo promotions de la revisión debe ser un arreglo

journal.cli.talent.daily.validation.suggest = validación: el campo entities de suggest debe ser un arreglo

journal.cli.talent.daily.write.contract = contrato de escritura diaria no admitido para { $talent }

journal.cli.talent.error.list.choice = uso: { $syntax }{ $line_break }{ $program }: error: argumento { $flag }: opción no válida: '{ $value }' (elige entre { $choices })

journal.cli.talent.error.list.unknown = uso: { $syntax }{ $line_break }{ $program }: error: argumentos no reconocidos: { $arguments }

journal.cli.talent.error.list.value = uso: { $syntax }{ $line_break }{ $program }: error: argumento { $flag }: se esperaba un argumento

journal.cli.talent.error.log.required = uso: { $syntax }{ $line_break }{ $program }: error: se requieren los siguientes argumentos: { $arguments }

journal.cli.talent.error.logs.count = uso: { $syntax }{ $line_break }{ $program }: error: argumento { $count_options }: valor entero no válido: '{ $value }'

journal.cli.talent.error.logs.value = uso: { $syntax }{ $line_break }{ $program }: error: argumento { $flag }: se esperaba un argumento

journal.cli.talent.error.passages.required = uso: { $syntax }{ $line_break }{ $program }: error: se requieren los siguientes argumentos: { $arguments }

journal.cli.talent.error.passages.unknown = uso: { $syntax }{ $line_break }{ $program }: error: argumentos no reconocidos: { $arguments }

journal.cli.talent.error.passages.value = uso: { $syntax }{ $line_break }{ $program }: error: argumento { $flag }: se esperaba un argumento

journal.cli.talent.error.root.choice = uso: { $syntax }{ $line_break }{ $program }: error: argumento { $subcommand }: opción no válida: '{ $value }' (elige entre { $choices })

journal.cli.talent.error.root.encoding = uso: { $syntax }{ $line_break }{ $program }: error: los argumentos no son válidos como { $encoding }

journal.cli.talent.error.root.unknown = uso: { $syntax }{ $line_break }{ $program }: error: argumentos no reconocidos: { $arguments }

journal.cli.talent.error.show.required = uso: { $syntax }{ $line_break }{ $program }: error: se requieren los siguientes argumentos: { $arguments }

journal.cli.talent.error.show.unknown = uso: { $syntax }{ $line_break }{ $program }: error: argumentos no reconocidos: { $arguments }

journal.cli.talent.error.show.value = uso: { $syntax }{ $line_break }{ $program }: error: argumento { $flag }: se esperaba un argumento

journal.cli.talent.facet.malformed = datos mal formados en { $path }

journal.cli.talent.facet.malformed.line = datos mal formados en { $path }, en la línea { $line }

journal.cli.talent.facet.object = la declaración de la faceta no es un objeto: { $path }

journal.cli.talent.floor.invalid.budget = falta o no es válido: { $budget }

journal.cli.talent.floor.mismatch = { $entries ->
    [one] no coincide el número de entradas: se analizó { $entries } entrada, pero hay { $sources ->
        [one] { $sources } fuente de entrada
        [many] { $sources } fuentes de entrada
       *[other] { $sources } fuentes de entrada
    }
    [many] no coincide el número de entradas: se analizaron { $entries } entradas, pero hay { $sources ->
        [one] { $sources } fuente de entrada
        [many] { $sources } fuentes de entrada
       *[other] { $sources } fuentes de entrada
    }
   *[other] no coincide el número de entradas: se analizaron { $entries } entradas, pero hay { $sources ->
        [one] { $sources } fuente de entrada
        [many] { $sources } fuentes de entrada
       *[other] { $sources } fuentes de entrada
    }
}

journal.cli.talent.help.list = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }muestra este mensaje de ayuda y sal{ $line_break }{ $syntax_continuation_5 }{ $line_break }                        Filtra por tipo de programación{ $line_break }{ $syntax_continuation_7 }{ $line_break }                        Filtra por origen{ $line_break }{ $indent_2 }{ $disabled_option }{ $gap_12 }Incluye las indicaciones desactivadas{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Muestra la salida como { $jsonl }

journal.cli.talent.help.log = uso: { $syntax }{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }  { $id }ID del agente{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }muestra este mensaje de ayuda y sal{ $line_break }{ $indent_2 }{ $json_option }{ $gap_6 }Muestra en bruto { $jsonl }{ $line_break }{ $indent_2 }{ $full_option }{ $gap_6 }Expande los detalles del evento

journal.cli.talent.help.logs = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }  { $agent }Filtra a un agente específico{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }muestra este mensaje de ayuda y sal{ $line_break }{ $syntax_continuation_9 }{ $line_break }                        Número de ejecuciones a mostrar (predeterminado: 20){ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }Muestra solo las ejecuciones de este día{ $line_break }{ $indent_2 }{ $daily_option }{ $gap_15 }Muestra solo las ejecuciones con programación diaria{ $line_break }{ $indent_2 }{ $errors_option }{ $gap_14 }Muestra solo las ejecuciones con error{ $line_break }{ $indent_2 }{ $summary_option }{ $gap_13 }Muestra el resumen agrupado

journal.cli.talent.help.passages = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }                           nombre{ $line_break }{ $line_break }Copia pasajes exactos de la fuente que lee un talento de actividad.{ $line_break }Una unidad de fuente corta se conserva entera sin una llamada al modelo; una más larga{ $line_break }usa una completación para elegir qué copiar. No se escriben pasajes en{ $line_break }el diario; el uso de una completación se registra como el de cualquier otra.{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }  { $name }Nombre del talento de actividad, como work o conversation{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }muestra este mensaje de ayuda y sal{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Muestra el documento de pasajes como { $json }{ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }Día de la actividad{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_10 }Faceta de la actividad{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_9 }ID de actividad

journal.cli.talent.help.root = uso: { $syntax }{ $line_break }{ $line_break }Inspecciona la configuración de indicaciones de los talentos{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $syntax_continuation_5 }{ $line_break }    { $list }Lista las indicaciones agrupadas por programación{ $line_break }    { $show }Muestra los detalles de una indicación específica{ $line_break }    { $passages }Copia los pasajes de la fuente de una actividad{ $line_break }    { $logs }Muestra el registro reciente de ejecuciones del talento{ $line_break }    { $log }Muestra los eventos de una ejecución del agente{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }muestra este mensaje de ayuda y sal{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_9 }Activa la salida detallada{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_11 }Activa el registro de depuración

journal.cli.talent.help.show = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }                           nombre{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }  { $name }Nombre de la indicación{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }muestra este mensaje de ayuda y sal{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Muestra la salida como { $jsonl }{ $line_break }{ $indent_2 }{ $prompt_option }{ $gap_14 }Muestra el contexto completo de la indicación (modo de simulación){ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }Día para el contexto de la indicación{ $line_break }{ $indent_2 }{ $segment_option }{ $gap_2 }Segmento para indicaciones programadas por segmento{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_10 }Faceta para indicaciones de varias facetas{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_9 }ID de actividad para indicaciones programadas por actividad{ $line_break }{ $indent_2 }{ $query_option }{ $gap_10 }Consulta de ejemplo para agentes de herramientas{ $line_break }{ $indent_2 }{ $full_option }{ $gap_16 }Muestra el contenido completo sin truncar

journal.cli.talent.hook.briefing.date.overflow = desbordamiento de la fecha del resumen

journal.cli.talent.hook.briefing.failed = falló el paso previo del resumen de la mañana: { $error }

journal.cli.talent.hook.budget = la solicitud preparada no cabe en el presupuesto mínimo de entrada

journal.cli.talent.hook.daily.lookback = retrospección de la programación diaria fuera de { $range }

journal.cli.talent.hook.day.invalid = día no válido o no analizable: '{ $day }'

journal.cli.talent.hook.day.missing = falta el día

journal.cli.talent.hook.document.stream = no es un segmento de importación de documento

journal.cli.talent.hook.entity.minimum = el paquete de la entidad { $entity } tiene al menos { $count } caracteres; el máximo es { $maximum }

journal.cli.talent.hook.entity.name = falta el nombre de la entidad

journal.cli.talent.hook.entity.size = el paquete de la entidad { $entity } tiene { $count } caracteres; el máximo es { $maximum }

journal.cli.talent.hook.facet.missing = falta la faceta

journal.cli.talent.hook.facet.read = no se pudo leer la faceta { $facet }: { $error }

journal.cli.talent.hook.facets.list = no se pudieron listar las facetas: { $error }

journal.cli.talent.hook.facts.size = el contexto de hechos de la entidad tiene { $count } caracteres; el máximo es { $maximum }

journal.cli.talent.hook.failure = { $phase } hook '{ $stage }' del talento '{ $talent }': { $detail }

journal.cli.talent.hook.identities.read = no se pudieron leer las identidades de la entidad: { $error }

journal.cli.talent.hook.identity.object = la identidad de la entidad no es un objeto: { $path }

journal.cli.talent.hook.lookup.control = componente de ruta no válido '{ $candidate }': el nombre contiene un carácter de control

journal.cli.talent.hook.lookup.dotcomponent = componente de ruta no válido '{ $candidate }': { $dot } y { $parent } no son nombres válidos

journal.cli.talent.hook.lookup.empty = componente de ruta no válido '{ $candidate }': el nombre está vacío

journal.cli.talent.hook.lookup.kind = { $path } no es un directorio ({ $kind })

journal.cli.talent.hook.lookup.layout = el diseño directo exige el flujo "{ $default_stream }", se recibió { $stream }

journal.cli.talent.hook.lookup.rootorprefix = componente de ruta no válido '{ $candidate }': el nombre debe ser un solo componente relativo

journal.cli.talent.hook.lookup.separator = componente de ruta no válido '{ $candidate }': el nombre contiene un separador de ruta

journal.cli.talent.hook.possible.no.prompt = el talento no tiene indicación

journal.cli.talent.hook.resolution.absent = la ambigüedad resuelta { $ambiguity } nombra la entidad { $entity }, ausente de esta llamada de resolución

journal.cli.talent.hook.resolution.hidden = la ambigüedad resuelta { $ambiguity } nombra la entidad oculta { $entity }

journal.cli.talent.hook.review.changed = las entradas de la revisión cambiaron durante la preparación

journal.cli.talent.hook.stream.layout = el diseño directo exige el flujo "{ $default_stream }", se recibió "{ $stream }"

journal.cli.talent.hook.suggest.size = el contexto de sugerencias de entidades tiene { $count } caracteres; el máximo es { $maximum }

journal.cli.talent.hook.suggestions.array = al JSON de sugerencias le falta el arreglo { $entities }

journal.cli.talent.hook.suggestions.json = el JSON de sugerencias no es válido en { $path }: { $error }

journal.cli.talent.hook.suggestions.missing = falta el artefacto de sugerencias: { $path }

journal.cli.talent.hook.sunday = el día de la solicitud { $day } no es un domingo

journal.cli.talent.hook.transcript.bytes = los bytes de la transcripción no son UTF-8 válido

journal.cli.talent.hook.transcript.read = no se pudo leer la transcripción { $file }: { $error }

journal.cli.talent.hook.transcript.utf8 = la transcripción { $file } no es UTF-8: { $error }

journal.cli.talent.hook.week.incomplete = la semana que termina el { $end } no ha terminado antes de hoy { $today }

journal.cli.talent.last.age = hace { $age }

journal.cli.talent.last.age.duration = hace { $age } ({ $duration })

journal.cli.talent.list.empty = No se encontraron indicaciones que coincidan con los filtros.

journal.cli.talent.list.group.activity = actividad:

journal.cli.talent.list.group.cadence = cadencia:

journal.cli.talent.list.group.daily = diaria:

journal.cli.talent.list.group.segment = segmento:

journal.cli.talent.list.group.unscheduled = sin programación:

journal.cli.talent.list.group.weekly = semanal:

journal.cli.talent.list.header.last = ÚLTIMA EJECUCIÓN

journal.cli.talent.list.header.name = NOMBRE

journal.cli.talent.list.header.tags = ETIQUETAS

journal.cli.talent.list.header.title = TÍTULO

journal.cli.talent.list.tag.disabled = desactivada

journal.cli.talent.list.tag.fail = FAIL

journal.cli.talent.list.visible = { $count ->
    [one] { $count } indicación (desactivadas ocultas: { $hidden }, usa { $disabled_flag })
    [many] { $count } indicaciones (desactivadas ocultas: { $hidden }, usa { $disabled_flag })
   *[other] { $count } indicaciones (desactivadas ocultas: { $hidden }, usa { $disabled_flag })
}

journal.cli.talent.log.event.thinking = razonamiento

journal.cli.talent.log.event.tool = herramienta

journal.cli.talent.log.event.updated = actualizado

journal.cli.talent.log.missing = No se encontró la ejecución del talento: { $run }

journal.cli.talent.log.tokens = { $result } [{ $input } entrada/{ $output } salida]

journal.cli.talent.logs.date = { $month } { $day_time }

journal.cli.talent.logs.day.invalid = Formato de { $day_flag } no válido: { $day }. Se esperaba { $day_format }.

journal.cli.talent.logs.total = total

journal.cli.talent.outcome.facet.write = FacetEntityWriteError: { $error }

journal.cli.talent.outcome.object = el resultado no es un objeto JSON

journal.cli.talent.outcome.observation.write = ObservationWriteError: { $error }

journal.cli.talent.outcome.observer.entities = el campo entities no es una lista

journal.cli.talent.outcome.observer.parse = no se pudo analizar el resultado como JSON

journal.cli.talent.outcome.review.arrays = al resultado le faltan los arreglos esperados

journal.cli.talent.outcome.review.candidate = EntityReviewCandidateError: { $error }

journal.cli.talent.outcome.review.parse = el JSON no es válido

journal.cli.talent.passages.generate = los pasajes de la fuente: no se pudo llegar a generate: { $error }

journal.cli.talent.passages.generate.missing = los pasajes de la fuente: no se pudo llegar a generate: falta el ejecutable hermano { $path }

journal.cli.talent.passages.generate.parent = los pasajes de la fuente: no se pudo llegar a generate: el ejecutable actual no tiene directorio padre

journal.cli.talent.passages.incomplete = algunas unidades de fuente no tienen pasajes; mira el resultado de cada unidad

journal.cli.talent.passages.unavailable = pasajes de la fuente no disponibles: { $detail }

journal.cli.talent.passages.unit.failed = # unidad { $index } · { $kind } · sin pasajes: { $reason }

journal.cli.talent.passages.unit.failed.time = # unidad { $index } · { $kind } · { $time } · sin pasajes: { $reason }

journal.cli.talent.passages.unit.not.attempted = # unidad { $index } · { $kind } · no se intentó

journal.cli.talent.passages.unit.not.attempted.time = # unidad { $index } · { $kind } · { $time } · no se intentó

journal.cli.talent.passages.unit.passages = { $count ->
    [one] # unidad { $index } · { $kind } · { $count } pasaje
    [many] # unidad { $index } · { $kind } · { $count } pasajes
   *[other] # unidad { $index } · { $kind } · { $count } pasajes
}

journal.cli.talent.passages.unit.passages.time = { $count ->
    [one] # unidad { $index } · { $kind } · { $time } · { $count } pasaje
    [many] # unidad { $index } · { $kind } · { $time } · { $count } pasajes
   *[other] # unidad { $index } · { $kind } · { $time } · { $count } pasajes
}

journal.cli.talent.passages.verbatim = Literal de la fuente:

journal.cli.talent.path.absolute = la ruta del diario debe ser relativa

journal.cli.talent.path.backslash = la ruta del diario debe usar barras diagonales

journal.cli.talent.path.component = la ruta del diario contiene un componente no válido

journal.cli.talent.path.day = el día debe ser { $day_format }

journal.cli.talent.path.empty = la ruta del diario no debe estar vacía

journal.cli.talent.path.escape = { $relative } sale de la raíz del diario (se resolvió a { $path })

journal.cli.talent.path.unreadable = la ruta del diario no se puede leer

journal.cli.talent.prepare.configuration.name.declares.an.invalid.mobile.local.timeout = la configuración '{ $name }' declara un valor no válido para { $timeout }

journal.cli.talent.prepare.invalid.config.must.have.type.day.or.prompt = Configuración no válida: debe tener { $type }, { $day } o { $prompt }

journal.cli.talent.prepare.invalid.config.segment.or.span.requires.day = Configuración no válida: { $segment } o { $span } exige { $day }

journal.cli.talent.prepare.invalid.talent.type.kind = tipo de talento no válido '{ $kind }'

journal.cli.talent.prepare.memory.incomplete = el contexto de recuerdos privados está incompleto

journal.cli.talent.prepare.request.field.field.is.retired = el campo de la solicitud '{ $field }' está retirado

journal.cli.talent.prepare.request.overrides.field.for.talent.name.are.not.allowed = No se permiten las anulaciones de '{ $field }' para el talento '{ $name }' ({ $declared } != { $requested })

journal.cli.talent.prepare.request.overrides.for.field.are.not.allowed = no se permiten anulaciones de '{ $field }'; el proveedor y el modelo de razonamiento solo se configuran en Razonamiento

journal.cli.talent.prepare.request.overrides.for.field.are.not.allowed.budget = no se permiten anulaciones de '{ $field }'; los presupuestos de un talento son suyos

journal.cli.talent.prepare.segment.missing = { $count ->
    [one] No se encontró el directorio del segmento: { $segments }
    [many] No se encontraron los directorios de los segmentos: { $segments }
   *[other] No se encontraron los directorios de los segmentos: { $segments }
}

journal.cli.talent.prepare.talent.name.declares.field.in.frontmatter = el talento '{ $name }' declara '{ $field }' en el frontmatter; el proveedor y el modelo de razonamiento solo se configuran en Razonamiento

journal.cli.talent.prepare.talent.name.declares.outbound.approval.in.frontmatter = el talento '{ $name }' declara { $outbound_approval } en el frontmatter; este campo es solo de la configuración de lanzamiento y no puede venir de una definición de talento

journal.cli.talent.prepare.talent.name.not.found = no se encontró el talento '{ $name }'

journal.cli.talent.prepare.talent.request.missing.name = a la solicitud del talento le falta el nombre

journal.cli.talent.preview.all.lines = (usa { $full_flag } para ver las { $count } líneas)

journal.cli.talent.preview.banner = Vista previa de lo que enviaría este talento. Usa el mismo armado que una ejecución real; no se llama al modelo.

journal.cli.talent.preview.effective = Indicación efectiva para: { $talent }

journal.cli.talent.preview.empty = (vacío)

journal.cli.talent.preview.instruction = INSTRUCCIÓN

journal.cli.talent.preview.no.day = No se indicó un día, así que esta vista previa no incluye las grabaciones de un día. Pasa { $day_flag } { $day_format } para incluirlas.

journal.cli.talent.preview.omitted = { $count ->
    [one] ... ({ $count } línea omitida)
    [many] ... ({ $count } líneas omitidas)
   *[other] ... ({ $count } líneas omitidas)
}

journal.cli.talent.preview.omitted.legacy = ... ({ $count } líneas omitidas)

journal.cli.talent.preview.prestep = La vista previa de la indicación no puede armar este talento: un paso previo obligatorio no está disponible.

journal.cli.talent.preview.recovery.choose.a.completed.owner.activity.not.an.anticipated.or.cogi = Elige una actividad completada del propietario, no un registro anticipated ni cogitate.

journal.cli.talent.preview.recovery.choose.a.talent.configured.for.this.activity.kind = Elige un talento configurado para este tipo de actividad.

journal.cli.talent.preview.recovery.choose.a.talent.that.loads.its.activity.s.sources = Elige un talento que cargue las fuentes de su actividad.

journal.cli.talent.preview.recovery.choose.a.talent.that.reads.its.activity.source.without.a.pre = Elige un talento que lea la fuente de su actividad sin un paso previo.

journal.cli.talent.preview.recovery.choose.a.talent.whose.required.pre.step.is.available.in.the = Elige un talento cuyo paso previo obligatorio esté disponible en el tiempo de ejecución nativo.

journal.cli.talent.preview.recovery.choose.a.talent.whose.schedule.is.activity = Elige un talento cuya programación sea { $schedule }

journal.cli.talent.preview.recovery.choose.an.activity.id.stored.in.the.requested.day.and.facet = Elige un ID de actividad guardado en el día y la faceta pedidos.

journal.cli.talent.preview.recovery.choose.an.activity.with.at.least.one.recorded.segment = Elige una actividad con al menos un segmento grabado.

journal.cli.talent.preview.recovery.choose.either.activity.or.segment.not.both = Elige { $activity_flag } o { $segment_flag }, no los dos.

journal.cli.talent.preview.recovery.correct.the.activity.preview.input.then.retry.detail = Corrige la entrada de la vista previa de la actividad y vuelve a intentarlo: { $detail }

journal.cli.talent.preview.recovery.correct.the.talent.configuration.detail = Corrige la configuración del talento: { $detail }

journal.cli.talent.preview.recovery.make.the.selected.activity.and.talent.eligible.then.retry = Haz que la actividad y el talento seleccionados sean elegibles, y vuelve a intentarlo.

journal.cli.talent.preview.recovery.make.the.selected.talent.eligible.then.retry = Haz que el talento seleccionado sea elegible y vuelve a intentarlo.

journal.cli.talent.preview.recovery.pass.day.yyyymmdd.facet.name.and.activity.id = Pasa { $day_flag } { $yyyymmdd }, { $facet_flag } { $name } y { $activity_flag } ID.

journal.cli.talent.preview.recovery.pass.day.yyyymmdd.with.activity = Pasa { $day_flag } { $yyyymmdd } con { $activity_flag }.

journal.cli.talent.preview.recovery.pass.facet.name.with.activity = Pasa { $facet_flag } { $name } con { $activity_flag }.

journal.cli.talent.preview.recovery.remove.duplicate.records.with.this.id.in.the.requested.day.a = Quita los registros duplicados con este ID en el día y la faceta pedidos.

journal.cli.talent.preview.recovery.repair.the.activity.span.so.every.segment.is.a.non.empty.str = Repara el span de la actividad para que cada segmento sea una cadena no vacía.

journal.cli.talent.preview.recovery.restore.a.readable.activity.record.file.then.retry.detail = Restaura un archivo legible del registro de actividad y vuelve a intentarlo: { $detail }

journal.cli.talent.preview.recovery.restore.a.readable.segment.directory.then.retry.detail = Restaura un directorio de segmento legible y vuelve a intentarlo: { $detail }

journal.cli.talent.preview.recovery.restore.readable.segment.directories.then.retry.detail = Restaura directorios de segmento legibles y vuelve a intentarlo: { $detail }

journal.cli.talent.preview.recovery.restore.the.selected.activity.segment.then.retry = Restaura el segmento de actividad seleccionado y vuelve a intentarlo.

journal.cli.talent.preview.skipped = Este talento no se ejecutaría: { $reason }

journal.cli.talent.show.body = cuerpo:

journal.cli.talent.show.body.lines = { $count ->
    [one] { $count } línea
    [many] { $count } líneas
   *[other] { $count } líneas
}

journal.cli.talent.show.hook = La indicación '{ $talent }' es una indicación de hook y no se puede ejecutar directamente.

journal.cli.talent.show.missing = No se encontró la indicación: { $talent }

journal.cli.talent.show.missing.path = No se encontró la indicación: { $talent }{ $line_break }  se buscó en: { $path }

journal.cli.talent.show.variables = variables:

journal.cli.talent.startup.executable = el talento falló: no se pudo inspeccionar el ejecutable actual: { $error }

journal.cli.talent.startup.package = el talento falló: no se pudo ubicar el paquete instalado { $package }

journal.cli.talent.work.duration = duración de la fuente Work no válida: { $error }

journal.cli.talent.work.duration.range = La duración de la fuente Work está fuera de rango

journal.cli.talent.work.interval = intervalo de la fuente Work no válido: { $error }

journal.cli.talent.work.interval.range = El intervalo de la fuente Work está fuera de rango

journal.cli.talent.work.key = segmento de la fuente Work no válido: { $segment }

journal.cli.talent.work.read = no se pueden leer los resultados del segmento de Work: { $error }

journal.cli.talent.work.unavailable = El segmento de la fuente Work no está disponible: { $day }/{ $segment }

journal.cli.top.crashed.attempts = { $name } (intentos: { $attempts })

journal.cli.top.crashed.crashed = { $name } (Falló; intentos: { $attempts })

journal.cli.top.crashed.heading.attention = Servicios que necesitan atención:

journal.cli.top.crashed.heading.crashed = Falló:

journal.cli.top.crashed.retrying = { $name } (Reintentando; intentos: { $attempts })

journal.cli.top.error.apply = no se pudo aplicar la terminal: { $detail }

journal.cli.top.error.journal.config = configuración del diario: { $detail }

journal.cli.top.error.screen = no se pudo entrar en la pantalla de la terminal: { $detail }

journal.cli.top.error.stdin.not.tty = stdin no es una terminal

journal.cli.top.error.stdout.not.tty = stdout no es una terminal

journal.cli.top.error.terminal = fallo de la terminal: { $detail }

journal.cli.top.error.transport = fallo de transporte: { $detail }

journal.cli.top.error.width = no se pudo obtener el ancho de la terminal: { $detail }

journal.cli.top.error.width.unavailable = el ancho de la terminal no está disponible

journal.cli.top.footer.empty = q: Salir

journal.cli.top.footer.multi = ↑/↓: Navegar  r: Reiniciar  q: Salir

journal.cli.top.footer.single = r: Reiniciar  q: Salir

journal.cli.top.header.last.log = Último registro

journal.cli.top.header.mb = MB

journal.cli.top.header.percent = %

journal.cli.top.header.pid = PID

journal.cli.top.header.runtime = Tiempo en ejecución

journal.cli.top.header.service = Servicio

journal.cli.top.header.task = Tarea

journal.cli.top.header.uptime = Tiempo activo

journal.cli.top.malformed.events = eventos malformados: { $count } ({ $reason })

journal.cli.top.observe.describe = describe

journal.cli.top.observe.idle = [INACTIVO]

journal.cli.top.observe.idle.locked = [INACTIVO] bloqueado

journal.cli.top.observe.live.badge = [EN VIVO]

journal.cli.top.observe.recent = Recientes: { $segments }

journal.cli.top.observe.screencast = grabación de pantalla { $elapsed }

journal.cli.top.observe.tmux.badge = [TMUX]

journal.cli.top.observe.tmux.captures = { $quantity ->
    [one] { $count } fotograma
    [many] { $count } fotogramas
   *[other] { $count } fotogramas
    }

journal.cli.top.observe.transcribe = transcribe

journal.cli.top.observe.voice = voz { $hits }

journal.cli.top.queued = en cola: { $commands }

journal.cli.top.reconnecting = (reconectando)

journal.cli.top.section.observe = Recepción

journal.cli.top.section.think = Razonamiento

journal.cli.top.section.thinking.health = Estado del razonamiento

journal.cli.top.task.status.failed = falló

journal.cli.top.task.status.gone = ausente

journal.cli.top.task.status.ok = ok

journal.cli.top.think.agents = { $total_quantity ->
    [one] { $completed }/{ $total } ejecución
    [many] { $completed }/{ $total } ejecuciones
   *[other] { $completed }/{ $total } ejecuciones
    }

journal.cli.top.think.last.failed = { $failed } falló

journal.cli.top.think.last.prefix = Última: { $success } ok

journal.cli.top.think.segments = seg { $completed }/{ $total }

journal.cli.top.thinking.checking = (revisando)

journal.cli.top.thinking.unavailable = (estado no disponible)

journal.cli.top.title = administrador de actividad de solstone

journal.cli.top.unknown = desconocido

journal.cli.top.waiting.services = (en espera de los servicios)

journal.cli.top.waiting.status = (en espera del estado)

journal.cli.top.waiting.think = (en espera del razonamiento)
