journal.cli.journal.archive.argument = argumento inesperado

journal.cli.journal.archive.day.exclusive = { $day_flag } no se puede combinar con { $from_flag } ni con { $to_flag }

journal.cli.journal.archive.day.invalid = { $token_day } debe ser una fecha { $token_yyyymmdd } real

journal.cli.journal.archive.day.required = { $day_flag } requiere { $token_yyyymmdd }

journal.cli.journal.archive.encode.finish.body = cuerpo de cierre del archivo comprimido: { $detail }

journal.cli.journal.archive.encode.finish.finalize = cierre final del archivo comprimido: { $detail }

journal.cli.journal.archive.encode.member = escritura del archivo comprimido { $member }: { $detail }

journal.cli.journal.archive.encode.metadata = metadatos no válidos del archivo comprimido { $field } { $value }: { $reason }

journal.cli.journal.archive.encode.source = origen del archivo comprimido: { $detail }

journal.cli.journal.archive.encode.write = escritura del archivo comprimido: { $detail }

journal.cli.journal.archive.encode.writer = escritor del archivo comprimido no válido: { $reason }

journal.cli.journal.archive.export.help = Uso: { $syntax }{ $line_break }{ $line_break }crea un { $token_zip } de tu diario, con la configuración y las credenciales.{ $line_break }quedan afuera los registros de procesamiento, los índices que se pueden reconstruir, las cachés, los bloqueos y los archivos temporales.{ $line_break }con un filtro de fecha, solo se incluyen los días elegidos.

journal.cli.journal.archive.exports.unsafe = directorio de exportaciones no seguro: { $path }

journal.cli.journal.archive.fact.archive.output.contains.a.null.byte = la salida del archivo comprimido contiene un byte nulo

journal.cli.journal.archive.fact.archive.output.ends.with.a.separator = la salida del archivo comprimido termina con un separador

journal.cli.journal.archive.fact.archive.output.has.no.file.name = la salida del archivo comprimido no tiene nombre de archivo

journal.cli.journal.archive.fact.archive.output.has.no.parent = la salida del archivo comprimido no tiene directorio padre

journal.cli.journal.archive.fact.archive.output.must.be.one.file.name = la salida del archivo comprimido debe ser un solo nombre de archivo

journal.cli.journal.archive.fact.archive.output.parent.is.missing.or.unsafe = falta el directorio padre de la salida del archivo comprimido, o no es seguro

journal.cli.journal.archive.fact.archive.output.path.is.not.valid.utf8 = la ruta de salida del archivo comprimido no es { $token_utf_8 } válido

journal.cli.journal.archive.fact.archive.output.uses.an.unsupported.prefix = la salida del archivo comprimido usa un prefijo no compatible

journal.cli.journal.archive.fact.archive.parent.component.is.invalid = el componente padre del archivo comprimido no es válido

journal.cli.journal.archive.fact.block.device = dispositivo de bloques

journal.cli.journal.archive.fact.character.device = dispositivo de caracteres

journal.cli.journal.archive.fact.could.not.determine.writer.position = no se pudo determinar la posición del escritor

journal.cli.journal.archive.fact.could.not.read.writer.flags = no se pudieron leer los flags del escritor

journal.cli.journal.archive.fact.could.not.stat.writer = no se pudo hacer stat del escritor

journal.cli.journal.archive.fact.directory = directorio

journal.cli.journal.archive.fact.fifo = fifo

journal.cli.journal.archive.fact.filesystem.root.is.not.a.directory = la raíz del sistema de archivos no es un directorio

journal.cli.journal.archive.fact.has.no.parent.directory = no tiene directorio padre

journal.cli.journal.archive.fact.must.be.a.regular.file = debe ser un archivo regular

journal.cli.journal.archive.fact.must.be.empty = debe estar vacío

journal.cli.journal.archive.fact.must.be.opened.for.writing = debe estar abierto para escritura

journal.cli.journal.archive.fact.must.be.positioned.at.the.start = debe estar al inicio

journal.cli.journal.archive.fact.must.be.valid.utf8 = debe ser { $token_utf_8 } válido

journal.cli.journal.archive.fact.must.name.one.normal.file.leaf = debe nombrar una sola hoja de archivo normal

journal.cli.journal.archive.fact.must.not.be.opened.in.append.mode = no debe estar abierto en modo de adición

journal.cli.journal.archive.fact.must.resolve.to.an.absolute.path = debe resolverse en una ruta absoluta

journal.cli.journal.archive.fact.other = otro

journal.cli.journal.archive.fact.regular.file = archivo regular

journal.cli.journal.archive.fact.relative.archive.output.requires.an.absolute.cwd = la salida relativa del archivo comprimido requiere un directorio de trabajo absoluto

journal.cli.journal.archive.fact.socket = socket

journal.cli.journal.archive.fact.symlink = enlace simbólico

journal.cli.journal.archive.fact.utf8.member.path.exceeds.zip.u16.byte.limit = la ruta del miembro en { $token_utf_8 } supera el límite de bytes { $token_u16 } de { $token_zip }

journal.cli.journal.archive.from.invalid = { $token_from } debe ser una fecha { $token_yyyymmdd } real

journal.cli.journal.archive.from.required = { $from_flag } requiere { $token_yyyymmdd }

journal.cli.journal.archive.import.ambiguities = el tipo no coincide en { $token_entities_ambiguities_jsonl }

journal.cli.journal.archive.import.appeared = { $relative } apareció mientras la importación estaba en curso

journal.cli.journal.archive.import.busy = el bloqueo de fusión del archivo comprimido está ocupado en { $path }: { $detail }

journal.cli.journal.archive.import.cleanup = falló la limpieza de la extracción { $path }: { $detail }

journal.cli.journal.archive.import.compressed = { $count ->
    [one] el archivo comprimido { $path } es de { $bytes } byte; el máximo es { $maximum }
    [many] el archivo comprimido { $path } es de { $bytes } bytes; el máximo es { $maximum }
   *[other] el archivo comprimido { $path } es de { $bytes } bytes; el máximo es { $maximum }
}

journal.cli.journal.archive.import.coordinate = coordenada de grabación planificada no válida

journal.cli.journal.archive.import.directory.remains = queda el directorio

journal.cli.journal.archive.import.encrypted.extraction = entrada cifrada del archivo comprimido durante la extracción: { $entry }

journal.cli.journal.archive.import.encrypted.validation = entrada cifrada del archivo comprimido durante la validación: { $entry }

journal.cli.journal.archive.import.entity = falló la fusión de la entidad { $entity_id }: { $detail }

journal.cli.journal.archive.import.expanded = { $count ->
    [one] el archivo comprimido se expande a { $bytes } byte; el máximo es { $maximum }
    [many] el archivo comprimido se expande a { $bytes } bytes; el máximo es { $maximum }
   *[other] el archivo comprimido se expande a { $bytes } bytes; el máximo es { $maximum }
}

journal.cli.journal.archive.import.extract = falló la extracción de { $archive } en { $dir }: { $detail }

journal.cli.journal.archive.import.facet = falló la fusión de la faceta { $facet }: { $detail }

journal.cli.journal.archive.import.handling = al manejar { $path }: { $detail }

journal.cli.journal.archive.import.hidden.archive = no se pueden leer los días ocultos de datos del cuerpo de este archivo comprimido, así que este archivo comprimido no se puede importar

journal.cli.journal.archive.import.hidden.details = { $detail }; { $warning }

journal.cli.journal.archive.import.hidden.target = este diario no puede leer qué días de datos del cuerpo ocultaste, así que este archivo comprimido no se puede importar

journal.cli.journal.archive.import.hidden.warning = { $days ->
    [one] 1 día de datos del cuerpo estaba oculto en el archivo comprimido, así que también está oculto en este diario; puedes desocultarlo en la página del cuerpo.
    [many] { $days } días de datos del cuerpo estaban ocultos en el archivo comprimido, así que también están ocultos en este diario; puedes desocultarlos en la página del cuerpo.
   *[other] { $days } días de datos del cuerpo estaban ocultos en el archivo comprimido, así que también están ocultos en este diario; puedes desocultarlos en la página del cuerpo.
}

journal.cli.journal.archive.import.invalid = archivo comprimido no válido { $path }: { $detail }

journal.cli.journal.archive.import.kind = el tipo no coincide en { $relative }

journal.cli.journal.archive.import.legacy = un archivo { $token_tgz } o { $token_tar_gz } no es compatible. si es un archivo comprimido de transferencia antiguo, vuelve a exportar ese día como { $token_zip } desde el diario de origen y fusiona ese, o fusiona este archivo en un diario nuevo y vacío en una versión { $token_2_x } anterior, exporta ese diario como { $token_zip } y fusiona ese { $token_zip } en tu diario

journal.cli.journal.archive.import.lock = falló el bloqueo de fusión del archivo comprimido en { $path }: { $detail }

journal.cli.journal.archive.import.log = falló la escritura del registro de decisiones en { $path }: { $detail }

journal.cli.journal.archive.import.magic = firma no reconocida del archivo comprimido

journal.cli.journal.archive.import.marker = falló la actualización del marcador del flujo después de publicar el contenido del archivo comprimido: { $detail }; el contenido publicado no se revirtió

journal.cli.journal.archive.import.marker.day = { $day }: { $detail }

journal.cli.journal.archive.import.merge = falló la fusión de la importación en { $path }: { $detail }

journal.cli.journal.archive.import.missing = no se encontró el archivo comprimido: { $path }

journal.cli.journal.archive.import.name = nombre de ruta ausente o que no es { $token_utf_8 }

journal.cli.journal.archive.import.owner = espera a quien lo tiene, o inspecciona o quita solo los metadatos del propietario en { $path } que estén verificados como desactualizados

journal.cli.journal.archive.import.path.encoding = { $path } no es { $token_utf_8 }

journal.cli.journal.archive.import.path.outside = { $path } está fuera del diario

journal.cli.journal.archive.import.publish = falló la publicación de la fusión del archivo comprimido: { $detail }

journal.cli.journal.archive.import.redirect.entity = este diario fusionó '{ $source_id }' en '{ $survivor }', y las fusiones no se pueden deshacer, así que este archivo comprimido no se puede importar

journal.cli.journal.archive.import.redirect.facet = este diario tenía una faceta llamada '{ $facet }' que se fusionó o a la que se le cambió el nombre, y ese nombre lleva allí ahora, así que este archivo comprimido no se puede importar

journal.cli.journal.archive.import.segment = falló la fusión del segmento en { $path }: { $detail }

journal.cli.journal.archive.import.segment.appeared = el segmento apareció antes de la publicación del archivo comprimido

journal.cli.journal.archive.import.segment.changed = cambió el segmento publicado del archivo comprimido

journal.cli.journal.archive.import.segment.symlink = enlace simbólico en el segmento

journal.cli.journal.archive.import.space = espacio libre insuficiente en { $path }: { $available } disponibles, { $required } requeridos

journal.cli.journal.archive.import.stage = falló la escritura de preparación en { $path }: { $detail }

journal.cli.journal.archive.import.stage.changed = cambió el directorio padre de preparación

journal.cli.journal.archive.import.stopped = la importación se detuvo

journal.cli.journal.archive.import.symlink = enlace simbólico

journal.cli.journal.archive.import.traversal = ruta absoluta o con recorrido hacia arriba

journal.cli.journal.archive.import.unambiguous = el archivo comprimido no tiene una raíz del diario sin ambigüedad

journal.cli.journal.archive.import.undo = { $detail }; deshacer incompleto: { $failures }

journal.cli.journal.archive.import.undo.coordinate = coordenada { $relative }: { $detail }

journal.cli.journal.archive.import.undo.restore = restaurar { $relative }: { $detail }

journal.cli.journal.archive.import.undo.rewrite = reescribir el mapa de identidad: { $detail }

journal.cli.journal.archive.import.undo.unlink = desenlazar { $relative }: { $detail }

journal.cli.journal.archive.import.unimplemented = import-sources: no implementado: { $module }

journal.cli.journal.archive.import.unsafe.extraction = entrada no segura del archivo comprimido durante la extracción: { $entry }: { $detail }

journal.cli.journal.archive.import.unsafe.validation = entrada no segura del archivo comprimido durante la validación: { $entry }: { $detail }

journal.cli.journal.archive.incomplete = { $committed } aplicados, { $skipped } omitidos, { $failed } fallidos

journal.cli.journal.archive.ios = no disponible en { $token_ios }

journal.cli.journal.archive.merge.help = Uso: { $syntax }

journal.cli.journal.archive.metadata.reason.invalid.calendar.date.or.time = fecha u hora de calendario no válida

journal.cli.journal.archive.metadata.reason.invalid.day = día no válido

journal.cli.journal.archive.metadata.reason.invalid.hour = hora no válida

journal.cli.journal.archive.metadata.reason.invalid.minute = minuto no válido

journal.cli.journal.archive.metadata.reason.invalid.month = mes no válido

journal.cli.journal.archive.metadata.reason.invalid.second = segundo no válido

journal.cli.journal.archive.metadata.reason.must.contain.digits.in.numeric.positions = debe tener dígitos en las posiciones numéricas

journal.cli.journal.archive.metadata.reason.must.not.be.empty = no debe estar vacío

journal.cli.journal.archive.metadata.reason.must.not.contain.ascii.control.characters = no debe contener caracteres de control ASCII

journal.cli.journal.archive.metadata.reason.must.use.yyyy.mm.ddthh.mm.ssz = debe usar YYYY-MM-DDTHH:MM:SSZ

journal.cli.journal.archive.metadata.reason.second.must.be.in.seconds = el segundo debe estar en 0..=59

journal.cli.journal.archive.metadata.reason.year.must.be.in.years = el año debe estar en 1980..=2107

journal.cli.journal.archive.negative.identity = la identidad de origen es negativa

journal.cli.journal.archive.negative.size = el tamaño del archivo regular es negativo

journal.cli.journal.archive.out.required = { $out_flag } requiere { $token_path }

journal.cli.journal.archive.output.parent = la salida del archivo comprimido no tiene directorio padre

journal.cli.journal.archive.output.tree = la salida está dentro del árbol { $family } exportado

journal.cli.journal.archive.preview = Fusión del archivo comprimido (días: { $days }): 0 aplicados, { $skipped } omitidos, 0 fallidos.

journal.cli.journal.archive.publish.cleanup = limpiar el archivo temporal del archivo comprimido: { $detail }

journal.cli.journal.archive.publish.createtemp = crear el archivo temporal del archivo comprimido: { $detail }

journal.cli.journal.archive.publish.publish = publicar el archivo comprimido: { $detail }

journal.cli.journal.archive.publish.syncdirectory = sincronizar el directorio del archivo comprimido: { $detail }

journal.cli.journal.archive.publish.synctemp = sincronizar el archivo temporal del archivo comprimido: { $detail }

journal.cli.journal.archive.range = { $from_flag } no debe ser posterior a { $to_flag }

journal.cli.journal.archive.root.name = la raíz del diario no tiene nombre de archivo

journal.cli.journal.archive.root.parent = la raíz del diario no tiene directorio padre

journal.cli.journal.archive.scratch.name = no se pudo asignar un archivo temporal de trabajo exclusivo del archivo comprimido

journal.cli.journal.archive.skipped = { $token_solstone_journal_archive_export }: elementos de nivel superior omitidos: { $skipped }

journal.cli.journal.archive.source.budget = raíz del diario no compatible { $path }: el origen del archivo comprimido supera el cupo de inventario que admite

journal.cli.journal.archive.source.changed = cambió el origen del diario en { $member }

journal.cli.journal.archive.source.io.list.journal.directory = listar el directorio del diario: { $detail }

journal.cli.journal.archive.source.io.list.journal.directory.member = listar el directorio del diario { $member }: { $detail }

journal.cli.journal.archive.source.io.member = { $operation } { $member }: { $detail }

journal.cli.journal.archive.source.io.open.journal.directory.for.listing = abrir el directorio del diario para listarlo: { $detail }

journal.cli.journal.archive.source.io.open.journal.directory.for.listing.member = abrir el directorio del diario para listarlo { $member }: { $detail }

journal.cli.journal.archive.source.io.open.journal.directory.member = abrir el directorio del diario { $member }: { $detail }

journal.cli.journal.archive.source.io.open.journal.file = abrir el archivo del diario: { $detail }

journal.cli.journal.archive.source.io.open.journal.file.member = abrir el archivo del diario { $member }: { $detail }

journal.cli.journal.archive.source.io.open.retained.journal.root = abrir la raíz conservada del diario: { $detail }

journal.cli.journal.archive.source.io.open.retained.journal.root.member = abrir la raíz conservada del diario { $member }: { $detail }

journal.cli.journal.archive.source.io.read.inventoried.file = leer el archivo inventariado: { $detail }

journal.cli.journal.archive.source.io.read.inventoried.file.member = leer el archivo inventariado { $member }: { $detail }

journal.cli.journal.archive.source.io.read.regular.file.size = leer el tamaño del archivo regular: { $detail }

journal.cli.journal.archive.source.io.read.regular.file.size.member = leer el tamaño del archivo regular { $member }: { $detail }

journal.cli.journal.archive.source.io.read.source.file.identity.member = leer la identidad del archivo de origen { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.directory = stat del directorio inventariado: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.directory.member = stat del directorio inventariado { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.file = stat del archivo inventariado: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.file.member = stat del archivo inventariado { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.journal.entry = stat del elemento del diario: { $detail }

journal.cli.journal.archive.source.io.stat.journal.entry.member = stat del elemento del diario { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.directory = stat del directorio inventariado abierto: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.directory.member = stat del directorio inventariado abierto { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.file = stat del archivo inventariado abierto: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.file.member = stat del archivo inventariado abierto { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.directory = stat del directorio del diario abierto: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.directory.member = stat del directorio del diario abierto { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.file = stat del archivo del diario abierto: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.file.member = stat del archivo del diario abierto { $member }: { $detail }

journal.cli.journal.archive.source.missing = { $token_source } no existe: { $path }

journal.cli.journal.archive.source.regular = { $token_source } debe ser un archivo regular

journal.cli.journal.archive.source.required = { $token_source } es obligatorio

journal.cli.journal.archive.source.unsafe = elemento del diario no seguro { $member }: { $kind }

journal.cli.journal.archive.source.windows.arm.recursive.namespace.witness = armar el testigo recursivo del espacio de nombres: { $detail }

journal.cli.journal.archive.source.windows.arm.recursive.namespace.witness.member = armar el testigo recursivo del espacio de nombres { $member }: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root = tomar prestada la raíz admitida del diario: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.member = tomar prestada la raíz admitida del diario { $member }: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.namespace.watch = tomar prestado el watch del espacio de nombres de la raíz admitida del diario: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.namespace.watch.member = tomar prestado el watch del espacio de nombres de la raíz admitida del diario { $member }: { $detail }

journal.cli.journal.archive.source.windows.cancel.recursive.namespace.witness = cancelar el testigo recursivo del espacio de nombres: { $detail }

journal.cli.journal.archive.source.windows.cancel.recursive.namespace.witness.member = cancelar el testigo recursivo del espacio de nombres { $member }: { $detail }

journal.cli.journal.archive.source.windows.check.recursive.namespace.witness = revisar el testigo recursivo del espacio de nombres: { $detail }

journal.cli.journal.archive.source.windows.check.recursive.namespace.witness.member = revisar el testigo recursivo del espacio de nombres { $member }: { $detail }

journal.cli.journal.archive.source.windows.create.recursive.namespace.witness.event = crear el evento del testigo recursivo del espacio de nombres: { $detail }

journal.cli.journal.archive.source.windows.create.recursive.namespace.witness.event.member = crear el evento del testigo recursivo del espacio de nombres { $member }: { $detail }

journal.cli.journal.archive.source.windows.drain.recursive.namespace.witness = vaciar el testigo recursivo del espacio de nombres: { $detail }

journal.cli.journal.archive.source.windows.drain.recursive.namespace.witness.member = vaciar el testigo recursivo del espacio de nombres { $member }: { $detail }

journal.cli.journal.archive.source.windows.list.retained.directory = listar el directorio conservado: { $detail }

journal.cli.journal.archive.source.windows.list.retained.directory.member = listar el directorio conservado { $member }: { $detail }

journal.cli.journal.archive.source.windows.open.retained.relative.entry = abrir el elemento relativo conservado: { $detail }

journal.cli.journal.archive.source.windows.open.retained.relative.entry.member = abrir el elemento relativo conservado { $member }: { $detail }

journal.cli.journal.archive.source.windows.parse.retained.directory.listing = interpretar el listado del directorio conservado: { $detail }

journal.cli.journal.archive.source.windows.parse.retained.directory.listing.member = interpretar el listado del directorio conservado { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.attributes = consultar los atributos del elemento conservado: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.attributes.member = consultar los atributos del elemento conservado { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.identity = consultar la identidad del elemento conservado: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.identity.member = consultar la identidad del elemento conservado { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.metadata = consultar los metadatos del elemento conservado: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.metadata.member = consultar los metadatos del elemento conservado { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.type = consultar el tipo del elemento conservado: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.type.member = consultar el tipo del elemento conservado { $member }: { $detail }

journal.cli.journal.archive.source.windows.read.observed.file = leer el archivo observado: { $detail }

journal.cli.journal.archive.source.windows.read.observed.file.member = leer el archivo observado { $member }: { $detail }

journal.cli.journal.archive.source.windows.reverify.retained.relative.directory.identity = volver a verificar la identidad del directorio relativo conservado: { $detail }

journal.cli.journal.archive.source.windows.reverify.retained.relative.directory.identity.member = volver a verificar la identidad del directorio relativo conservado { $member }: { $detail }

journal.cli.journal.archive.source.windows.size.checked.read.buffer = dimensionar el búfer de lectura verificada: { $detail }

journal.cli.journal.archive.source.windows.size.checked.read.buffer.member = dimensionar el búfer de lectura verificada { $member }: { $detail }

journal.cli.journal.archive.source.windows.trace.retained.relative.directory.listing.open = rastrear la apertura del listado del directorio relativo conservado: { $detail }

journal.cli.journal.archive.source.windows.trace.retained.relative.directory.listing.open.member = rastrear la apertura del listado del directorio relativo conservado { $member }: { $detail }

journal.cli.journal.archive.source.windows.unsupported.arm.recursive.namespace.witness = raíz del diario no compatible { $path }: armar el testigo recursivo del espacio de nombres

journal.cli.journal.archive.source.windows.unsupported.cancel.recursive.namespace.witness = raíz del diario no compatible { $path }: cancelar el testigo recursivo del espacio de nombres

journal.cli.journal.archive.source.windows.unsupported.check.recursive.namespace.witness = raíz del diario no compatible { $path }: revisar el testigo recursivo del espacio de nombres

journal.cli.journal.archive.source.windows.unsupported.drain.recursive.namespace.witness = raíz del diario no compatible { $path }: vaciar el testigo recursivo del espacio de nombres

journal.cli.journal.archive.source.windows.unsupported.wait.for.recursive.namespace.witness = raíz del diario no compatible { $path }: esperar el testigo recursivo del espacio de nombres

journal.cli.journal.archive.source.windows.wait.for.recursive.namespace.witness = esperar el testigo recursivo del espacio de nombres: { $detail }

journal.cli.journal.archive.source.windows.wait.for.recursive.namespace.witness.member = esperar el testigo recursivo del espacio de nombres { $member }: { $detail }

journal.cli.journal.archive.space.overflow = desbordamiento del espacio disponible en disco

journal.cli.journal.archive.space.platform = la inspección del espacio en disco no es compatible con esta plataforma

journal.cli.journal.archive.summary = Fusión del archivo comprimido: { $committed } aplicados, { $skipped } omitidos, { $failed } fallidos.

journal.cli.journal.archive.target.changed = cambió la ruta de salida del archivo comprimido durante la exportación: { $path }

journal.cli.journal.archive.target.collision = la salida del archivo comprimido ya existe: { $path }

journal.cli.journal.archive.target.invalid = salida del archivo comprimido no válida { $path }: { $reason }

journal.cli.journal.archive.target.io = falló { $operation } para { $path }: { $detail }

journal.cli.journal.archive.target.io.open.archive.output.filesystem.root = no se pudo abrir la raíz del sistema de archivos de la salida del archivo comprimido para { $path }: { $detail }

journal.cli.journal.archive.target.io.open.archive.output.parent = no se pudo abrir el directorio padre de la salida del archivo comprimido para { $path }: { $detail }

journal.cli.journal.archive.target.io.retain.admitted.archive.output.parent = no se pudo conservar el directorio padre admitido de la salida del archivo comprimido para { $path }: { $detail }

journal.cli.journal.archive.target.io.revalidate.admitted.archive.output.parent = no se pudo volver a validar el directorio padre admitido de la salida del archivo comprimido para { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.admitted.archive.output.parent = no se pudo hacer stat del directorio padre admitido de la salida del archivo comprimido para { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.archive.output.filesystem.root = no se pudo hacer stat de la raíz del sistema de archivos de la salida del archivo comprimido para { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.archive.output.final.name = no se pudo hacer stat del nombre final de la salida del archivo comprimido para { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.archive.output.parent = no se pudo hacer stat del directorio padre de la salida del archivo comprimido para { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.opened.archive.output.parent = no se pudo hacer stat del directorio padre abierto de la salida del archivo comprimido para { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.retained.archive.output.parent = no se pudo hacer stat del directorio padre conservado de la salida del archivo comprimido para { $path }: { $detail }

journal.cli.journal.archive.target.unsafe = la salida del archivo comprimido es un { $kind } no seguro: { $path }

journal.cli.journal.archive.target.windows.changed = cambió la salida del archivo comprimido: { $path }

journal.cli.journal.archive.target.windows.io = salida del archivo comprimido { $path }: { $detail }

journal.cli.journal.archive.target.windows.unsafe = salida no segura del archivo comprimido { $path }: { $reason }

journal.cli.journal.archive.temp.name = no se pudo asignar un nombre temporal único para el archivo comprimido

journal.cli.journal.archive.to.invalid = { $token_to } debe ser una fecha { $token_yyyymmdd } real

journal.cli.journal.archive.to.required = { $to_flag } requiere { $token_yyyymmdd }

journal.cli.journal.archive.zero.write = el escritor del archivo comprimido devolvió cero bytes en una escritura que no estaba vacía

journal.cli.journal.facet.argument.ios = no está disponible en { $platform }

journal.cli.journal.facet.argument.unexpected = argumento inesperado

journal.cli.journal.facet.content.unsafe = contenido de faceta no seguro: { $path }

journal.cli.journal.facet.doctor.adopted.heading = Facetas huérfanas adoptadas

journal.cli.journal.facet.doctor.adoption.failed = { $destination } (falló la adopción: { $error })

journal.cli.journal.facet.doctor.adoption.unmerged = { $folders_remaining ->
    [one] { $destination } (falló la adopción: { $error }; { $unmerged } no se fusionó)
    [many] { $destination } (falló la adopción: { $error }; { $unmerged } no se fusionaron)
   *[other] { $destination } (falló la adopción: { $error }; { $unmerged } no se fusionaron)
    }

journal.cli.journal.facet.doctor.argument.merge = { $merge_flag } requiere { $adopt_flag }

journal.cli.journal.facet.doctor.collision.heading = Colisiones de archivos regulares

journal.cli.journal.facet.doctor.empty = Nada que informar.

journal.cli.journal.facet.doctor.failed.heading = Facetas huérfanas fallidas

journal.cli.journal.facet.doctor.fix.ios = { $fix_flag } no está disponible en { $platform }

journal.cli.journal.facet.doctor.folders.hidden = Las carpetas cuyo nombre empieza con un punto nunca son facetas, así que se ignoran estas declaraciones de faceta: { $folders }

journal.cli.journal.facet.doctor.group.ambiguous = { $members }: con nombre parecido a { $facets }; cambia el nombre de la carpeta para que coincida con una y luego ejecútalo con { $adopt_flag } { $merge_flag }

journal.cli.journal.facet.doctor.group.damaged = { $members }: con nombre parecido a { $facets }, cuya { $declaration } hay que reparar primero

journal.cli.journal.facet.doctor.help = Uso: { $syntax }{ $line_break }{ $retire_syntax }

journal.cli.journal.facet.doctor.history.failed = falló la revisión del registro de acciones: { $error }

journal.cli.journal.facet.doctor.history.merged = { $name } se fusionó en { $destination }

journal.cli.journal.facet.doctor.history.recorded = Nombres de facetas retiradas registrados en los registros de acciones:

journal.cli.journal.facet.doctor.history.renamed = { $name } se renombró a { $destination }

journal.cli.journal.facet.doctor.history.reused = A estas facetas se les cambió el nombre o se fusionaron antes, y sus nombres se dieron después a una faceta nueva, así que el material más antiguo que las nombra no se puede distinguir. No se cambió nada: { $names }

journal.cli.journal.facet.doctor.history.unknown = Los registros de acciones mencionan estos nombres, pero no a dónde fueron: { $names }. Si algún material todavía nombra uno, regístralo con { $retire_arguments } o { $deleted_flag }.

journal.cli.journal.facet.doctor.history.unrecorded = Nombres de facetas que los registros de acciones muestran como retirados, pero que no están registrados (ejecútalo con { $fix_flag } para registrarlos):

journal.cli.journal.facet.doctor.leftovers.failed = no se pudieron leer las carpetas sobrantes de la fusión: { $error }

journal.cli.journal.facet.doctor.leftovers.kept = Carpetas sobrantes de una fusión de facetas que pueden tener la única copia de algunos archivos. No las elimines; ejecútalo con { $fix_flag } para moverlas a la papelera: { $folders }

journal.cli.journal.facet.doctor.leftovers.moved = Se movieron a la papelera las carpetas sobrantes de una fusión de facetas, tal como estaban: { $folders }

journal.cli.journal.facet.doctor.leftovers.safe = Carpetas sobrantes de una fusión de facetas interrumpida, que se pueden eliminar cuando no haya una fusión en curso: { $folders }

journal.cli.journal.facet.doctor.link.duplicate.open = { $facet }: '{ $id }' está enlazada desde más de una carpeta ({ $folders })

journal.cli.journal.facet.doctor.link.duplicate.repaired = { $facet }: '{ $id }' estaba enlazada desde más de una carpeta ({ $folders })

journal.cli.journal.facet.doctor.link.merged.open = { $folder_count ->
    [one] { $facet }: '{ $id }' se fusionó en '{ $successor }', y la carpeta { $folders } todavía la enlaza
    [many] { $facet }: '{ $id }' se fusionó en '{ $successor }', y las carpetas { $folders } todavía la enlazan
   *[other] { $facet }: '{ $id }' se fusionó en '{ $successor }', y las carpetas { $folders } todavía la enlazan
    }

journal.cli.journal.facet.doctor.link.merged.repaired = { $folder_count ->
    [one] { $facet }: '{ $id }' se fusionó en '{ $successor }', y la carpeta { $folders } la enlazaba
    [many] { $facet }: '{ $id }' se fusionó en '{ $successor }', y las carpetas { $folders } la enlazaban
   *[other] { $facet }: '{ $id }' se fusionó en '{ $successor }', y las carpetas { $folders } la enlazaban
    }

journal.cli.journal.facet.doctor.link.misnamed.open = { $facet }: '{ $id }' está enlazada desde la carpeta { $folders }, no desde una carpeta llamada '{ $id }'

journal.cli.journal.facet.doctor.link.misnamed.repaired = { $facet }: '{ $id }' estaba enlazada desde la carpeta { $folders }, no desde una carpeta llamada '{ $id }'

journal.cli.journal.facet.doctor.link.unreadable = { $facet }: la carpeta { $folders } tiene un enlace que no se puede leer ni usar

journal.cli.journal.facet.doctor.link.unreadable.check = { $facet }: la carpeta { $folders } tiene un enlace que no se puede leer ni usar; { $fix_flag } lo deja como está

journal.cli.journal.facet.doctor.link.unreadable.undecided = { $facet }: la carpeta { $folders } tiene un enlace que no se puede leer ni usar ({ $fix_flag } no puede saber qué entidad enlaza, así que lo deja)

journal.cli.journal.facet.doctor.links.check = Enlaces de entidad por reparar (ejecútalo con { $fix_flag } para reparar los que se pueda):

journal.cli.journal.facet.doctor.links.check.failed = falló la revisión del enlace de entidad: { $error }

journal.cli.journal.facet.doctor.links.left = Enlaces de entidad dejados como están:

journal.cli.journal.facet.doctor.links.repair.failed = falló la reparación del enlace de entidad: { $error }

journal.cli.journal.facet.doctor.links.repaired = Enlaces de entidad reparados:

journal.cli.journal.facet.doctor.links.rescan = Ejecuta '{ $rescan_command }' para actualizar el índice.

journal.cli.journal.facet.doctor.lock.failed = no se tomó el bloqueo de la faceta: { $error }

journal.cli.journal.facet.doctor.maintenance.heading = Fallos de mantenimiento de fusiones aplicadas

journal.cli.journal.facet.doctor.merge.collision = { $source } -> { $destination }: { $path } (se conservó { $retained })

journal.cli.journal.facet.doctor.merge.failed = { $source } -> { $destination } (la fusión falló antes de aplicarse: { $error })

journal.cli.journal.facet.doctor.merge.ios = la fusión de facetas no está disponible en { $platform }

journal.cli.journal.facet.doctor.merged.heading = Facetas huérfanas fusionadas

journal.cli.journal.facet.doctor.merged.none = No se registró ni se fusionó ninguna faceta huérfana.

journal.cli.journal.facet.doctor.merges.incomplete = una o más fusiones de facetas huérfanas se aplicaron con fallos de mantenimiento

journal.cli.journal.facet.doctor.orphans.count = { $count ->
    [one] { $count } faceta huérfana encontrada. Ejecútalo con { $adopt_flag } para registrarla.
    [many] { $count } facetas huérfanas encontradas. Ejecútalo con { $adopt_flag } para registrarlas.
   *[other] { $count } facetas huérfanas encontradas. Ejecútalo con { $adopt_flag } para registrarlas.
    }

journal.cli.journal.facet.doctor.orphans.heading = Facetas huérfanas:

journal.cli.journal.facet.doctor.orphans.held = Los registros de acciones muestran que estos nombres de faceta se retiraron, así que sus carpetas no se registran; ejecútalo con { $fix_flag } para registrarlos: { $folders }

journal.cli.journal.facet.doctor.orphans.none = No se encontraron facetas huérfanas.

journal.cli.journal.facet.doctor.orphans.retired = Estas carpetas tienen contenido bajo nombres de faceta que se fusionaron o se renombraron a otra faceta, así que no se registran como facetas; sus archivos se dejan donde están: { $folders }

journal.cli.journal.facet.doctor.queue.failed = la reparación terminó, pero no se pudo poner en cola su actualización de búsqueda: { $error }

journal.cli.journal.facet.doctor.reference = - { $name }: { $count } ({ $first } a { $last })

journal.cli.journal.facet.doctor.references.command = Para que una vuelva a poder encontrarse, ejecuta: { $retire_command }

journal.cli.journal.facet.doctor.references.failed = falló la revisión de referencias: { $error }

journal.cli.journal.facet.doctor.references.missing = Hay segmentos que nombran estas facetas, que no existen y no están registradas como retiradas. Los agentes limitados a las facetas elegidas no pueden ver ese material; todo lo demás sí:

journal.cli.journal.facet.doctor.registered.none = No se registró ninguna faceta huérfana.

journal.cli.journal.facet.doctor.registration.failed = se registró { $slug }, pero no se pudo escribir su registro: { $error }

journal.cli.journal.facet.doctor.repair.failed = no se pudieron reparar los nombres de facetas retiradas: { $error }

journal.cli.journal.facet.doctor.repaired.count = { $repaired ->
    [one] { $repaired } faceta huérfana reparada. la búsqueda se está poniendo al día.
    [many] { $repaired } facetas huérfanas reparadas. la búsqueda se está poniendo al día.
   *[other] { $repaired } facetas huérfanas reparadas. la búsqueda se está poniendo al día.
    }

journal.cli.journal.facet.doctor.repaired.heading = Facetas huérfanas reparadas:

journal.cli.journal.facet.doctor.repairs.failed = falló la reparación de una o más facetas huérfanas

journal.cli.journal.facet.doctor.repairs.failed.count = { $repaired ->
    [one] { $failed_orphans ->
        [one] { $repaired } faceta huérfana reparada; { $failed_orphans } faceta huérfana falló. Ejecuta '{ $rescan_command }' para actualizar el índice.
        [many] { $repaired } faceta huérfana reparada; { $failed_orphans } facetas huérfanas fallaron. Ejecuta '{ $rescan_command }' para actualizar el índice.
       *[other] { $repaired } faceta huérfana reparada; { $failed_orphans } facetas huérfanas fallaron. Ejecuta '{ $rescan_command }' para actualizar el índice.
        }
    [many] { $failed_orphans ->
        [one] { $repaired } facetas huérfanas reparadas; { $failed_orphans } faceta huérfana falló. Ejecuta '{ $rescan_command }' para actualizar el índice.
        [many] { $repaired } facetas huérfanas reparadas; { $failed_orphans } facetas huérfanas fallaron. Ejecuta '{ $rescan_command }' para actualizar el índice.
       *[other] { $repaired } facetas huérfanas reparadas; { $failed_orphans } facetas huérfanas fallaron. Ejecuta '{ $rescan_command }' para actualizar el índice.
        }
   *[other] { $failed_orphans ->
        [one] { $repaired } facetas huérfanas reparadas; { $failed_orphans } faceta huérfana falló. Ejecuta '{ $rescan_command }' para actualizar el índice.
        [many] { $repaired } facetas huérfanas reparadas; { $failed_orphans } facetas huérfanas fallaron. Ejecuta '{ $rescan_command }' para actualizar el índice.
       *[other] { $repaired } facetas huérfanas reparadas; { $failed_orphans } facetas huérfanas fallaron. Ejecuta '{ $rescan_command }' para actualizar el índice.
        }
    }

journal.cli.journal.facet.doctor.repairs.maintenance.count = { $repaired ->
    [one] { $merges ->
        [one] { $repaired } faceta huérfana reparada; { $merges } fusión se aplicó, pero informó un fallo de mantenimiento después de aplicarse. Consulta 'Fallos de mantenimiento de fusiones aplicadas' más arriba. Ejecuta '{ $rescan_command }' para actualizar el índice.
        [many] { $repaired } faceta huérfana reparada; { $merges } fusiones se aplicaron, pero informaron un fallo de mantenimiento después de aplicarse. Consulta 'Fallos de mantenimiento de fusiones aplicadas' más arriba. Ejecuta '{ $rescan_command }' para actualizar el índice.
       *[other] { $repaired } faceta huérfana reparada; { $merges } fusiones se aplicaron, pero informaron un fallo de mantenimiento después de aplicarse. Consulta 'Fallos de mantenimiento de fusiones aplicadas' más arriba. Ejecuta '{ $rescan_command }' para actualizar el índice.
        }
    [many] { $merges ->
        [one] { $repaired } facetas huérfanas reparadas; { $merges } fusión se aplicó, pero informó un fallo de mantenimiento después de aplicarse. Consulta 'Fallos de mantenimiento de fusiones aplicadas' más arriba. Ejecuta '{ $rescan_command }' para actualizar el índice.
        [many] { $repaired } facetas huérfanas reparadas; { $merges } fusiones se aplicaron, pero informaron un fallo de mantenimiento después de aplicarse. Consulta 'Fallos de mantenimiento de fusiones aplicadas' más arriba. Ejecuta '{ $rescan_command }' para actualizar el índice.
       *[other] { $repaired } facetas huérfanas reparadas; { $merges } fusiones se aplicaron, pero informaron un fallo de mantenimiento después de aplicarse. Consulta 'Fallos de mantenimiento de fusiones aplicadas' más arriba. Ejecuta '{ $rescan_command }' para actualizar el índice.
        }
   *[other] { $merges ->
        [one] { $repaired } facetas huérfanas reparadas; { $merges } fusión se aplicó, pero informó un fallo de mantenimiento después de aplicarse. Consulta 'Fallos de mantenimiento de fusiones aplicadas' más arriba. Ejecuta '{ $rescan_command }' para actualizar el índice.
        [many] { $repaired } facetas huérfanas reparadas; { $merges } fusiones se aplicaron, pero informaron un fallo de mantenimiento después de aplicarse. Consulta 'Fallos de mantenimiento de fusiones aplicadas' más arriba. Ejecuta '{ $rescan_command }' para actualizar el índice.
       *[other] { $repaired } facetas huérfanas reparadas; { $merges } fusiones se aplicaron, pero informaron un fallo de mantenimiento después de aplicarse. Consulta 'Fallos de mantenimiento de fusiones aplicadas' más arriba. Ejecuta '{ $rescan_command }' para actualizar el índice.
        }
    }

journal.cli.journal.facet.doctor.retired.malformed = { $retired_path } no se puede analizar ({ $detail }). Ejecútalo con { $fix_flag } para reconstruirlo; no se libera ningún nombre que reserve.

journal.cli.journal.facet.doctor.retired.rebuilt = { $retired_path } no se pudo analizar ({ $detail }); se movió a { $path } y se reconstruyó.

journal.cli.journal.facet.doctor.retired.reserved = No se pudieron rastrear estos nombres y se registraron como eliminados: { $names }.

journal.cli.journal.facet.doctor.retired.unreadable = no se pudo leer { $retired_path } ({ $detail }). Revisa sus permisos; se dejó como está.

journal.cli.journal.facet.doctor.search.failed = no se pudo poner en cola la actualización de búsqueda: { $error }

journal.cli.journal.facet.doctor.search.queued = la búsqueda se está poniendo al día con los cambios de facetas.

journal.cli.journal.facet.doctor.search.skipped = La búsqueda no se actualizó, porque { $retired_path } todavía no se puede usar.

journal.cli.journal.facet.doctor.twins.heading = Carpetas con nombre parecido a una faceta que tienes:

journal.cli.journal.facet.doctor.twins.merge = Estas nunca se registran junto a ella. Ejecútalo con { $adopt_flag } { $merge_flag } para fusionarlas en ella; una fusión es permanente, y un archivo que las dos tienen conserva la copia de la faceta.

journal.cli.journal.facet.doctor.twins.preview = Para ver primero qué traería una: { $merge_command } { $source } { $into_flag } { $destination } { $dry_run_flag }

journal.cli.journal.facet.doctor.unclear.heading = Carpetas dejadas como están

journal.cli.journal.facet.doctor.variants.heading = Grupos de variantes de nombre:

journal.cli.journal.facet.doctor.variants.merge = Estas no se registraron. Ejecútalo con { $adopt_flag } { $merge_flag } para fusionarlas en una.

journal.cli.journal.facet.doctor.variants.register = Ejecútalo con { $adopt_flag } { $merge_flag } para fusionar las variantes de nombre antes de registrarlas.

journal.cli.journal.facet.entry.unsafe = elemento de faceta no seguro: { $path }

journal.cli.journal.facet.hidden.damaged = no se pueden leer los nombres ocultos en { $path }, así que no se cambió nada: { $detail }

journal.cli.journal.facet.hidden.empty = no se dio ningún nombre

journal.cli.journal.facet.hidden.field.text = { $key } no es texto

journal.cli.journal.facet.hidden.field.unknown = campo desconocido { $key }

journal.cli.journal.facet.hidden.names.list = { $names_field } no es una lista

journal.cli.journal.facet.hidden.row.empty = el nombre está vacío

journal.cli.journal.facet.hidden.row.error = nombre { $index }: { $detail }

journal.cli.journal.facet.hidden.row.object = no es un objeto

journal.cli.journal.facet.json.object = { $path } no es un objeto { $json_format }

journal.cli.journal.facet.link.conflict = { $path } tiene contenidos distintos en las carpetas que se combinan, así que no se combinó nada

journal.cli.journal.facet.link.file.encoding = { $path } no es texto { $encoding }

journal.cli.journal.facet.link.ids.exhausted = las notas se quedaron sin números de id

journal.cli.journal.facet.link.invalid.name = '{ $name }' no se puede usar como id de entidad

journal.cli.journal.facet.link.not.file = { $path } es una carpeta o un enlace, no un archivo, así que no se combinó nada

journal.cli.journal.facet.link.observation.corrupt = observación corrupta en { $source }: { $detail }

journal.cli.journal.facet.link.observation.malformed = observación mal formada en { $source }, en la línea { $line }: { $reason }

journal.cli.journal.facet.link.occupied = la carpeta '{ $id }' tiene otra entidad o no se puede leer

journal.cli.journal.facet.link.repair.destination = la carpeta de '{ $id }' en la faceta de destino tiene otra entidad o no se puede leer, así que no se cambió nada; ejecuta '{ $doctor_command }' para ver qué hay que reparar

journal.cli.journal.facet.link.repair.named = la carpeta de '{ $id }' en la faceta '{ $facet }' tiene otra entidad o no se puede leer, así que no se cambió nada; ejecuta '{ $doctor_command }' para ver qué hay que reparar

journal.cli.journal.facet.link.successor.missing = no se encuentra la entidad a la que se unió

journal.cli.journal.facet.log.action.queue = no se pudo poner en cola la actualización de búsqueda del registro de acciones: { $error }

journal.cli.journal.facet.log.detections.queue = las detecciones de faceta no quedaron en cola para la búsqueda facet={ $facet } cause={ $error }

journal.cli.journal.facet.log.trash.unfinished = elemento de la papelera { $id }: movido, se termina más tarde: { $error }

journal.cli.journal.facet.merge.argument.confirm = una fusión es permanente y no se puede deshacer; revísala con { $dry_run_flag } y vuelve a ejecutarla con { $yes_flag }

journal.cli.journal.facet.merge.argument.dest = { $into_flag } requiere { $destination_argument }

journal.cli.journal.facet.merge.argument.into = { $into_dest } es obligatorio

journal.cli.journal.facet.merge.argument.invalid = { $source_argument } o { $destination_argument } no es válido

journal.cli.journal.facet.merge.argument.source = { $source_argument } es obligatorio

journal.cli.journal.facet.merge.cleanup.failed = la fusión terminó, pero no se pudieron quitar sus carpetas sobrantes: { $error }; ejecuta '{ $repair_command }' para recogerlas

journal.cli.journal.facet.merge.collisions.kept = Las dos facetas tenían estos archivos. '{ $destination }' conservó su propia copia; la copia de '{ $source }' se conservó, y '{ $repair_command }' la mueve a la papelera:

journal.cli.journal.facet.merge.collisions.trashed = Las dos facetas tenían estos archivos. '{ $destination }' conservó su propia copia; la copia de '{ $source }' está en la papelera:

journal.cli.journal.facet.merge.complete = Se fusionó '{ $source }' en '{ $destination }'. Se completó la reconstrucción del índice.

journal.cli.journal.facet.merge.destination.absent = '{ $destination }' no es una faceta

journal.cli.journal.facet.merge.destination.id = '{ $destination }' todavía no tiene un id estable; ejecuta '{ $backfill_command }' primero

journal.cli.journal.facet.merge.destination.retired = '{ $destination }' está registrado como nombre de faceta retirado; ejecuta '{ $doctor_command }' para ver los detalles

journal.cli.journal.facet.merge.detections.failed = sus detecciones no quedaron en cola

journal.cli.journal.facet.merge.failed = falló la fusión de facetas

journal.cli.journal.facet.merge.help = Uso: { $syntax }

journal.cli.journal.facet.merge.preview.empty = Nada.

journal.cli.journal.facet.merge.preview.entities = { $count ->
    [one] { $count } entidad está enlazada en las dos facetas; quedaría con un enlace, conservando todas las notas.
    [many] { $count } entidades están enlazadas en las dos facetas; cada una quedaría con un enlace, conservando todas las notas.
   *[other] { $count } entidades están enlazadas en las dos facetas; cada una quedaría con un enlace, conservando todas las notas.
    }

journal.cli.journal.facet.merge.preview.fields = Campos de entidad en los que '{ $destination }' conservaría el suyo, distinto:

journal.cli.journal.facet.merge.preview.files = Archivos que las dos facetas tienen y que no se pueden combinar. '{ $destination }' conservaría su propia copia, y la copia de '{ $source }' iría a la papelera:

journal.cli.journal.facet.merge.preview.heading = Prueba en seco, no se cambió nada. Fusionar '{ $source }' en '{ $destination }' perdería:

journal.cli.journal.facet.merge.preview.notes = { $count ->
    [one] { $count } nota recibiría un número nuevo, después de las notas que ya están.
    [many] { $count } notas recibirían un número nuevo, después de las notas que ya están.
   *[other] { $count } notas recibirían un número nuevo, después de las notas que ya están.
    }

journal.cli.journal.facet.merge.preview.records = Registros que se descartarían porque se conserva un registro distinto con el mismo id (se conserva el primer registro de cada id, leyendo '{ $destination }' primero):

journal.cli.journal.facet.merge.preview.settings = La configuración de la { $declaration } de '{ $source }', que no se trasladaría.

journal.cli.journal.facet.merge.queue.failed = la reparación terminó, pero no se pudo poner en cola su actualización de búsqueda: { $error }

journal.cli.journal.facet.merge.scratch.failed = la prueba en seco no pudo quitar su copia temporal de trabajo { $path }: { $error }; no se cambió nada más, y esa carpeta se puede eliminar

journal.cli.journal.facet.merge.source.absent = '{ $source }' no es una faceta

journal.cli.journal.facet.merge.source.declared = '{ $source }' es una faceta declarada, no una carpeta huérfana

journal.cli.journal.facet.merge.source.only = '{ $source }' es la única faceta que no está silenciada, y un diario conserva al menos una; quita el silencio de '{ $destination }' primero

journal.cli.journal.facet.merge.source.retired = '{ $source }' es un nombre de faceta retirado

journal.cli.journal.facet.merge.trash.failed = la fusión terminó, pero no se pudieron mover a la papelera los archivos que reemplazó: { $error }; se conservan, y '{ $repair_command }' los mueve allí

journal.cli.journal.facet.name.encoding = el nombre de la faceta no es { $encoding }

journal.cli.journal.facet.name.unsafe = declaración de faceta no segura para { $slug }

journal.cli.journal.facet.notify.queue = no se pudo poner en cola la actualización de búsqueda: { $error }

journal.cli.journal.facet.notify.search = la búsqueda no se actualizó: { $reason }

journal.cli.journal.facet.path.encoding = { $path } no es { $encoding }

journal.cli.journal.facet.path.plain = { $path } no es una ruta simple

journal.cli.journal.facet.record.declaration.damaged = la declaración de la faceta está dañada: { $detail }

journal.cli.journal.facet.record.declaration.exists = la declaración de la faceta ya existe: { $path }

journal.cli.journal.facet.record.declaration.invalid = la declaración de la faceta no es un objeto: { $path }

journal.cli.journal.facet.record.declaration.missing = falta la declaración de la faceta: { $path }

journal.cli.journal.facet.record.declaration.unreadable = no se pudo leer la declaración de la faceta: { $detail }

journal.cli.journal.facet.record.id.duplicate = hay un identificador de faceta bien formado duplicado que ya está en el disco: { $id }

journal.cli.journal.facet.record.id.entropy = no se pudieron obtener bytes aleatorios para el identificador: { $error }

journal.cli.journal.facet.record.id.exhausted = se agotaron los reintentos para generar un identificador de faceta único

journal.cli.journal.facet.record.link.invalid = el enlace de entidad de la faceta no es un objeto: { $path }

journal.cli.journal.facet.record.malformed.activity = no se pueden leer las definiciones de actividad en { $path }, línea { $line }: { $reason }

journal.cli.journal.facet.record.malformed.observation = observación mal formada { $source } línea { $line }: { $reason }

journal.cli.journal.facet.record.marker.invalid = el marcador de finalización de la reparación del enlace de entidad de la faceta está vacío o mal formado: { $path }

journal.cli.journal.facet.record.minimum = la faceta '{ $facet }' es la única faceta activada; un diario conserva al menos una

journal.cli.journal.facet.record.missing.names = falta el objeto { $names_field }

journal.cli.journal.facet.record.name.retired = el nombre de faceta '{ $name }' perteneció a una faceta que se fusionó o a la que se le cambió el nombre, y ahora lleva a ella

journal.cli.journal.facet.record.not.object = no es un objeto { $json_format }

journal.cli.journal.facet.record.reason.content = se esperaba un objeto con contenido de texto no vacío

journal.cli.journal.facet.record.reason.history = el campo { $history_field } no es válido

journal.cli.journal.facet.record.reason.id = { $id_field } debe ser un entero sin signo

journal.cli.journal.facet.record.reason.json = { $json_format } no es válido

journal.cli.journal.facet.record.reason.object = se esperaba un objeto

journal.cli.journal.facet.record.reason.relation = se esperaba un objeto con contenido o relación de texto no vacío

journal.cli.journal.facet.record.reason.retired = el campo { $retired_field } no es válido

journal.cli.journal.facet.record.retired.conflict = el nombre de faceta '{ $name }' ya está registrado como retirado con un resultado distinto

journal.cli.journal.facet.record.retired.damaged = no se pudo leer { $retired_path } ({ $detail }); ejecuta '{ $repair_command }'

journal.cli.journal.facet.record.source.snapshot = instantánea capturada

journal.cli.journal.facet.retire.argument = { $retire_flag } necesita { $name_argument } y después { $into_facet } o { $deleted_flag }

journal.cli.journal.facet.retire.deleted = Se registró '{ $name }' como eliminado. la búsqueda se está poniendo al día.

journal.cli.journal.facet.retire.encoding = los nombres deben ser { $encoding }

journal.cli.journal.facet.retire.folder = '{ $name }' es una carpeta de este diario; solo se puede retirar un nombre que ya no existe

journal.cli.journal.facet.retire.merged = Se registró '{ $name }' como fusionado en '{ $target }'. la búsqueda se está poniendo al día.

journal.cli.journal.facet.retire.name = '{ $name }' no es un nombre de faceta

journal.cli.journal.facet.retire.target = '{ $target }' no es una faceta con un id estable; ejecuta '{ $backfill_command }' primero

journal.cli.journal.facet.transaction.exists = la ruta de la transacción ya existe: { $path }

journal.cli.journal.facet.transaction.rollback = la transacción falló y se revirtió: { $primary }

journal.cli.journal.facet.transaction.rollback.failed = la transacción falló ({ $primary }); la reversión también falló ({ $rollback_error })

journal.cli.journal.facet.trash.absent = eso ya no está en la papelera

journal.cli.journal.facet.trash.busy = eso ya se está recuperando o borrando

journal.cli.journal.facet.trash.io = no se pudo cambiar tu papelera: { $error }

journal.cli.journal.facet.trash.missing = no hay tal { $item } en tu diario

journal.cli.journal.facet.trash.unfinished = está en la papelera, que terminará de guardarlo

journal.cli.journal.facet.trash.unknown = esta compilación no puede recuperar eso

journal.cli.journal.help.overview = { $invocation } - el diario en esta computadora{ $line_break }{ $line_break }Uso: { $syntax }{ $line_break }{ $line_break }{ $old_command } se movió a '{ $invocation }'; usa ese comando para las operaciones locales del diario.{ $line_break }{ $line_break }Comandos locales:{ $line_break }{ $commands }{ $line_break }{ $line_break }Comandos de proceso:{ $line_break }{ $processes }{ $line_break }{ $line_break }Comandos locales del diario:{ $line_break }{ $local }{ $line_break }{ $line_break }Opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_4 }Mostrar esta ayuda{ $line_break }{ $syntax_continuation_17 }{ $line_break }{ $syntax_continuation_18 }

journal.cli.journal.help.path = uso: { $syntax }{ $line_break }{ $line_break }Imprimir la ruta resuelta del diario.

journal.cli.journal.help.root = uso: { $syntax }{ $line_break }{ $line_break }Imprimir la raíz del proyecto.

journal.cli.journal.help.setup.copy = uso: { $syntax }

journal.cli.journal.help.status = uso: { $syntax }{ $line_break }{ $line_break }Imprimir si el diario existe y cuántos días tiene.

journal.cli.journal.help.usage = Uso: { $syntax }

journal.cli.journal.helper.inspect = { $code }: no se pudo inspeccionar el ejecutable nativo del diario: { $error }

journal.cli.journal.helper.missing.mac = { $code }: { $path }. Vuelve a instalar la app del diario.

journal.cli.journal.helper.missing.other = { $code }: { $path }. Vuelve a instalar { $package }.

journal.cli.journal.helper.mode.mac = { $code }: { $path }. Vuelve a instalar la app del diario.

journal.cli.journal.helper.mode.other = { $code }: { $path }. Vuelve a instalar { $package }.

journal.cli.journal.host.path.utf8 = falló la resolución de la ruta nativa del diario: la ruta resuelta del diario no es UTF-8 válido

journal.cli.journal.host.status.absent = Diario: { $path }{ $line_break }Fuente: { $source }{ $line_break }Existe: no{ $line_break }Días: { $days }

journal.cli.journal.host.status.directory = falló el estado nativo del diario: la raíz del diario no es un directorio: { $path }

journal.cli.journal.host.status.exists = Diario: { $path }{ $line_break }Fuente: { $source }{ $line_break }Existe: sí{ $line_break }Días: { $days }

journal.cli.journal.host.status.inspect = falló el estado nativo del diario: no se pudo inspeccionar { $path }: { $error }

journal.cli.journal.host.status.utf8 = falló el estado nativo del diario: la ruta resuelta del diario no es UTF-8 válido

journal.cli.journal.indexer.argument.argument = argumento inesperado

journal.cli.journal.indexer.argument.budget = { $budget_seconds_flag } necesita un número positivo

journal.cli.journal.indexer.argument.drain = { $drain_flag } requiere { $apply_flag }

journal.cli.journal.indexer.argument.duplicate = argumento inesperado o duplicado

journal.cli.journal.indexer.argument.file = { $rescan_file_flag } requiere { $path }

journal.cli.journal.indexer.argument.ios = no está disponible en { $ios }

journal.cli.journal.indexer.argument.missing.agent = { $agent_flag } requiere { $agent }

journal.cli.journal.indexer.argument.missing.day = { $day_flag } requiere { $day }

journal.cli.journal.indexer.argument.missing.day.from = { $day_from_flag } requiere { $day }

journal.cli.journal.indexer.argument.missing.day.to = { $day_to_flag } requiere { $day }

journal.cli.journal.indexer.argument.missing.facet = { $facet_flag } requiere { $facet }

journal.cli.journal.indexer.argument.missing.limit = { $limit_flag } requiere un entero no negativo

journal.cli.journal.indexer.argument.missing.offset = { $offset_flag } requiere un entero no negativo

journal.cli.journal.indexer.argument.missing.stream = { $stream_flag } requiere { $stream }

journal.cli.journal.indexer.argument.missing.top = { $top_flag } requiere un entero no negativo

journal.cli.journal.indexer.argument.rescan.conflict = { $rescan_file_flag } no se puede combinar con { $rescan_flag } ni con { $rescan_full_flag }

journal.cli.journal.indexer.argument.unsupported = archivo no compatible

journal.cli.journal.indexer.build.building = compilación: compilando

journal.cli.journal.indexer.build.complete = compilación: completa

journal.cli.journal.indexer.build.unknown = compilación: desconocida

journal.cli.journal.indexer.calendar.at = línea { $line }: { $reason }

journal.cli.journal.indexer.calendar.import = notas de calendario en { $rel } omitidas: { $error }

journal.cli.journal.indexer.calendar.line.repeat = la línea { $line } de la nota de calendario repite un evento

journal.cli.journal.indexer.calendar.line.skip = línea { $line } de la nota de calendario omitida: { $reason }

journal.cli.journal.indexer.calendar.segment = notas de calendario omitidas: { $rel } no está en un segmento

journal.cli.journal.indexer.calendar.settings = el evento de calendario en { $path } se lista cuando se pueda leer el archivo de configuración: { $error }

journal.cli.journal.indexer.calendar.source = las notas de calendario solo se leen de flujos de calendario y de importaciones de calendario: { $rel }

journal.cli.journal.indexer.calendar.unlisted = nota de calendario en { $path } no listada: { $reason }

journal.cli.journal.indexer.calendar.validation.account.must.be.a.string.or.null = { $account_field } debe ser una cadena o { $null_key }

journal.cli.journal.indexer.calendar.validation.all.day.must.be.true.or.false = { $all_day_field } debe ser { $true_key } o { $false_key }

journal.cli.journal.indexer.calendar.validation.attendee.status.must.be.a.string.or.null = asistente { $status_field } debe ser una cadena o { $null_key }

journal.cli.journal.indexer.calendar.validation.attendee.you.must.be.true.or.false = asistente { $you_field } debe ser { $true_key } o { $false_key }

journal.cli.journal.indexer.calendar.validation.attendees.must.be.an.array = { $attendees_field } debe ser un arreglo

journal.cli.journal.indexer.calendar.validation.change.must.be.added.changed.or.removed = { $change_field } debe ser { $added_changed_or_removed_key }

journal.cli.journal.indexer.calendar.validation.changed.at.falls.outside.the.segment.window = { $changed_at_field } queda fuera de la ventana del segmento

journal.cli.journal.indexer.calendar.validation.changed.at.must.be.a.time.with.an.offset = { $changed_at_field } debe ser una hora con desfase

journal.cli.journal.indexer.calendar.validation.each.attendee.must.be.an.object = cada asistente debe ser un objeto

journal.cli.journal.indexer.calendar.validation.end.cannot.be.read.as.a.date.or.time = { $end_field } no se puede leer como fecha ni como hora

journal.cli.journal.indexer.calendar.validation.end.is.before.start = { $end_field } es anterior al inicio

journal.cli.journal.indexer.calendar.validation.end.must.be.a.string.or.null = { $end_field } debe ser una cadena o { $null_key }

journal.cli.journal.indexer.calendar.validation.event.and.occurrence.repeat.an.earlier.line = { $event_field } y la ocurrencia repiten una línea anterior

journal.cli.journal.indexer.calendar.validation.event.must.be.a.non.empty.string = { $event_field } debe ser una cadena no vacía

journal.cli.journal.indexer.calendar.validation.expanded.must.be.true.when.present = { $expanded_field } debe ser { $true_key } cuando está presente

journal.cli.journal.indexer.calendar.validation.field.missing = falta { $key }

journal.cli.journal.indexer.calendar.validation.field.type = { $key } debe ser una cadena o { $null_input }

journal.cli.journal.indexer.calendar.validation.file.encoding = el archivo no es { $encoding }

journal.cli.journal.indexer.calendar.validation.file.holds.no.notes = el archivo no tiene notas

journal.cli.journal.indexer.calendar.validation.file.lines = el archivo tiene más de 5 000 notas

journal.cli.journal.indexer.calendar.validation.file.size = el archivo tiene más de 8 { $unit }

journal.cli.journal.indexer.calendar.validation.filename = el nombre de archivo debe ser { $prefix } seguido de 64 dígitos hexadecimales en minúscula y { $suffix }

journal.cli.journal.indexer.calendar.validation.import.must.be.a.non.empty.string = { $import_field } debe ser una cadena no vacía

journal.cli.journal.indexer.calendar.validation.imported.at.must.be.a.time.with.an.offset = { $imported_at_key } debe ser una hora con desfase

journal.cli.journal.indexer.calendar.validation.line.is.empty = la línea está vacía

journal.cli.journal.indexer.calendar.validation.line.is.not.a.json.object = la línea no es un objeto { $json_key }

journal.cli.journal.indexer.calendar.validation.occurrence.cannot.be.read.as.a.date.or.time = { $occurrence_field } no se puede leer como fecha ni como hora

journal.cli.journal.indexer.calendar.validation.occurrence.must.be.a.string.or.null = { $occurrence_field } debe ser una cadena o { $null_key }

journal.cli.journal.indexer.calendar.validation.organizer.must.be.an.object.or.null = { $organizer_field } debe ser un objeto o { $null_key }

journal.cli.journal.indexer.calendar.validation.person.field = { $what } { $key } debe ser una cadena o { $null_input }

journal.cli.journal.indexer.calendar.validation.segment.grid = el segmento debe nombrar una ventana de 300 segundos en la cuadrícula del diario

journal.cli.journal.indexer.calendar.validation.segment.must.name.a.span.on.its.day = el segmento debe nombrar un tramo en su día

journal.cli.journal.indexer.calendar.validation.start.and.end.must.be.dates.exactly.when.all.day.is.true = { $start_field } y { $end_field } deben ser fechas exactamente cuando { $all_day_field } es { $true_key }

journal.cli.journal.indexer.calendar.validation.start.and.end.must.both.carry.an.offset.or.both.have.none = { $start_field } y { $end_field } deben tener desfase los dos, o ninguno

journal.cli.journal.indexer.calendar.validation.start.cannot.be.read.as.a.date.or.time = { $start_field } no se puede leer como fecha ni como hora

journal.cli.journal.indexer.calendar.validation.start.is.missing = falta { $start_field }

journal.cli.journal.indexer.calendar.validation.version = { $v_field } debe ser 1

journal.cli.journal.indexer.chunker.activity = versión de actividad de escritorio no compatible

journal.cli.journal.indexer.chunker.calendar = nota de calendario omitida: { $reason }

journal.cli.journal.indexer.chunker.lines = { $collapsed ->
    [one] Se contrajo el espacio en blanco en { $collapsed } línea Markdown demasiado larga durante la indexación
    [many] Se contrajo el espacio en blanco en { $collapsed } líneas Markdown demasiado largas durante la indexación
   *[other] Se contrajo el espacio en blanco en { $collapsed } líneas Markdown demasiado largas durante la indexación
    }

journal.cli.journal.indexer.classification.absent = clasificaciones: base de datos ausente

journal.cli.journal.indexer.classification.complete = clasificaciones: completas (procesadas { $processed }, sin clasificar { $unclassified })

journal.cli.journal.indexer.classification.incomplete = clasificaciones: sidecar incompleto en el cursor '{ $cursor }' (ejecuta '{ $repair }')

journal.cli.journal.indexer.classification.mismatch = clasificaciones: desajuste de cobertura en el cursor '{ $cursor }' (procesadas { $processed }, restantes { $remaining }, faltantes { $missing }, sin clasificar { $unclassified })

journal.cli.journal.indexer.classification.missing = { $missing ->
    [one] desajuste de cobertura de la clasificación de fragmentos: falta { $missing } fila
    [many] desajuste de cobertura de la clasificación de fragmentos: faltan { $missing } filas
   *[other] desajuste de cobertura de la clasificación de fragmentos: faltan { $missing } filas
    }

journal.cli.journal.indexer.classification.progress = clasificaciones: en curso en el cursor '{ $cursor }' (procesadas { $processed }, restantes { $remaining }, faltantes { $missing }, sin clasificar { $unclassified })

journal.cli.journal.indexer.classification.stall = se estancó el relleno de la clasificación de fragmentos: { $err }

journal.cli.journal.indexer.classification.stalled = clasificaciones: estancadas en '{ $path }' (procesadas { $processed }, restantes { $remaining }, faltantes { $missing }, sin clasificar { $unclassified })

journal.cli.journal.indexer.classification.unready = clasificaciones: no listas en el cursor '{ $cursor }' (ejecuta '{ $repair_command }')

journal.cli.journal.indexer.database.absent = índice: ausente

journal.cli.journal.indexer.database.files = archivos: { $discovered } en disco, { $indexed } indexados, { $current } actuales, { $stale } desactualizados, { $missing } faltantes, { $failed } fallidos, { $orphaned } huérfanos, { $retained } conservados hasta un reescaneo completo, { $ineligible_rows } ya no indexables pero todavía en la búsqueda, { $unreadable } ilegibles, { $ineligible } no indexables

journal.cli.journal.indexer.database.present = índice: presente

journal.cli.journal.indexer.database.sqlite = no es una base de datos { $sqlite }

journal.cli.journal.indexer.database.unreadable = índice: no se puede leer ({ $reason })

journal.cli.journal.indexer.discovery.listing = falló el recorrido glob: al intentar leer `{ $path }` hubo un error: { $error }

journal.cli.journal.indexer.discovery.pattern = patrón glob no válido: { $error }

journal.cli.journal.indexer.discovery.prefix = la ruta descubierta { $path } no está bajo la raíz { $root }

journal.cli.journal.indexer.discovery.relative = la ruta descubierta no es { $encoding } válido: { $path }

journal.cli.journal.indexer.discovery.root = la raíz glob no es { $encoding } válido: { $path }

journal.cli.journal.indexer.drain.result = { $processed ->
    [one] se aplicó { $processed } actualización de búsqueda; { $failed } fallida; { $remaining } { $remaining ->
        [one] queda
        [many] quedan
       *[other] quedan
    } en cola.
    [many] se aplicaron { $processed } actualizaciones de búsqueda; { $failed } fallidas; { $remaining } { $remaining ->
        [one] queda
        [many] quedan
       *[other] quedan
    } en cola.
   *[other] se aplicaron { $processed } actualizaciones de búsqueda; { $failed } fallidas; { $remaining } { $remaining ->
        [one] queda
        [many] quedan
       *[other] quedan
    } en cola.
    }

journal.cli.journal.indexer.edge.blob = la fila de arista contiene un valor blob no compatible

journal.cli.journal.indexer.edge.day = Día de arista no válido: { $day }

journal.cli.journal.indexer.edge.dst = la fila de arista requiere una cadena no vacía { $dst_field }

journal.cli.journal.indexer.edge.event = la marca de tiempo del evento está fuera del rango { $integer }

journal.cli.journal.indexer.edge.integer = el campo entero de arista { $field } está fuera del rango { $integer }

journal.cli.journal.indexer.edge.json = falló el análisis de la fuente de arista { $json } para { $path }: { $error }

journal.cli.journal.indexer.edge.kind = Tipo de arista desconocido: { $kind }

journal.cli.journal.indexer.edge.path = ruta de observaciones no válida: { $path }

journal.cli.journal.indexer.edge.payload = la carga de { $source } debe ser un objeto { $json }; se recibió { $value_type }

journal.cli.journal.indexer.edge.refresh = Se omite la actualización de aristas para { $rel }: { $error }

journal.cli.journal.indexer.edge.relation = a la relación de la observación le falta { $kind_field }

journal.cli.journal.indexer.edge.segment.day = Día de segmento no válido: { $day }

journal.cli.journal.indexer.edge.segment.key = Clave de segmento no válida: { $segment }

journal.cli.journal.indexer.edge.segment.time = la marca de tiempo del segmento está fuera del rango { $integer }

journal.cli.journal.indexer.edge.src = la fila de arista requiere una cadena no vacía { $src_field }

journal.cli.journal.indexer.edge.string = no se puede reproducir { $python } para el valor de { $field } de tipo { $value_type }

journal.cli.journal.indexer.edge.target = la relación de observación { $target_field } debe ser una cadena no vacía

journal.cli.journal.indexer.edge.type = el tipo de arista de { $source } debe ser una cadena; se recibió { $value_type }

journal.cli.journal.indexer.edge.unsupported = el campo de arista { $field } no admite { $value_type }

journal.cli.journal.indexer.error.file = falló rescan-file: { $detail }

journal.cli.journal.indexer.error.rebuild = falló la reconstrucción de aristas: { $detail }

journal.cli.journal.indexer.error.reset = falló el reinicio: { $detail }

journal.cli.journal.indexer.error.scan = falló el escaneo: { $detail }

journal.cli.journal.indexer.files.activity.membership.insert = falló la inserción de la pertenencia de la actividad para { $rel }: { $error }

journal.cli.journal.indexer.files.activity.provenance.insert = falló la inserción de la procedencia de la actividad para { $rel }: { $error }

journal.cli.journal.indexer.files.calendar.listing = falló el listado de calendario para { $rel }: { $error }

journal.cli.journal.indexer.files.calendar.note.insert = falló la inserción de la nota de calendario para { $rel }: { $error }

journal.cli.journal.indexer.files.chunk.group.offset.insert = falló la inserción del desplazamiento del grupo de fragmentos para { $rel }: { $error }

journal.cli.journal.indexer.files.chunk.insert = falló la inserción del fragmento para { $rel }: { $error }

journal.cli.journal.indexer.files.chunk.source.mapping = falló la asignación de fuente del fragmento para { $rel }: { $error }

journal.cli.journal.indexer.files.classification.insert = falló la inserción de la clasificación para { $rel }: { $error }

journal.cli.journal.indexer.files.content.read = falló la lectura del contenido para { $rel }: { $error }

journal.cli.journal.indexer.files.facet = falló el recorrido de la declaración de faceta para { $rel }: { $error }

journal.cli.journal.indexer.files.hidden = { $rel } queda fuera de la búsqueda por ahora: { $cause }

journal.cli.journal.indexer.files.memory.chain.serialization = falló la serialización de la cadena del recuerdo para { $rel }: { $error }

journal.cli.journal.indexer.files.memory.missing = falta el original del recuerdo ya validado para { $rel }

journal.cli.journal.indexer.files.memory.origin.serialization = falló la serialización del origen del recuerdo para { $rel }: { $error }

journal.cli.journal.indexer.files.memory.original.insert = falló la inserción del original del recuerdo para { $rel }: { $error }

journal.cli.journal.indexer.files.memory.utf = el original del recuerdo no es { $utf } para { $rel }: { $error }

journal.cli.journal.indexer.files.partial = { $rel } queda en parte fuera de la búsqueda por ahora: { $cause }

journal.cli.journal.indexer.files.screen.text.source.insert = falló la inserción de la fuente de texto de pantalla para { $rel }: { $error }

journal.cli.journal.indexer.generation.locked = generación: { $generation } (esta versión lee hasta { $known }; escribible: no)

journal.cli.journal.indexer.generation.writable = generación: { $generation } (esta versión lee hasta { $known }; escribible: sí)

journal.cli.journal.indexer.help.drain = Uso: { $syntax }{ $line_break }aplicar las actualizaciones de búsqueda en cola. { $force_flag } incluye los reintentos demorados.{ $line_break }{ $budget_seconds_flag } detiene el vaciado después de N segundos. una actualización de todo el diario no empieza cuando quedan menos de 300 segundos y sigue en cola.

journal.cli.journal.indexer.help.main = Uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $syntax_continuation_3 }{ $line_break }{ $syntax_continuation_4 }

journal.cli.journal.indexer.help.path = Uso: { $syntax }

journal.cli.journal.indexer.help.status = Uso: { $syntax }

journal.cli.journal.indexer.hidden.activity = no se puede saber si esta actividad está oculta: { $rel } no se puede leer: { $error }

journal.cli.journal.indexer.hidden.damaged = no se puede saber si esta actividad está oculta: { $path } tiene una línea que no es un registro

journal.cli.journal.indexer.hidden.days = no se puede saber qué días de datos de cuerpo están ocultos: { $path } no se puede leer

journal.cli.journal.indexer.hidden.trash = no se puede saber qué está rehaciendo el diario: { $error }

journal.cli.journal.indexer.hidden.week = no se puede saber qué queda fuera de esta página de la semana: { $path } no se puede leer

journal.cli.journal.indexer.hidden.weeks = no se puede saber qué queda fuera de las páginas de la semana: { $path } no se puede leer

journal.cli.journal.indexer.merge.commit = el índice está correcto y solo está sin compactar: no se pudo aplicar la fusión de { $fts }: { $error }

journal.cli.journal.indexer.merge.failed = el índice está correcto y solo está sin compactar: falló la fusión de { $fts }: { $error }

journal.cli.journal.indexer.merge.start = el índice está correcto y solo está sin compactar: no se pudo iniciar la transacción de fusión de { $fts }: { $error }

journal.cli.journal.indexer.migration.child = la migración del agregado del segmento conservó { $rel_segment }: el archivo hijo no está al día: { $rel }

journal.cli.journal.indexer.migration.discovery = la migración del agregado del segmento conservó { $rel_segment }: falló el descubrimiento de talento: { $error }

journal.cli.journal.indexer.migration.files = quedan archivos del índice después de la migración: { $paths }

journal.cli.journal.indexer.migration.mtime = la migración del agregado del segmento conservó { $rel_segment }: falló la lectura de mtime para { $rel }: { $error }

journal.cli.journal.indexer.migration.retained = la migración del agregado del segmento conservó { $rel }: { $error }

journal.cli.journal.indexer.migration.unclassified = la migración del agregado del segmento conservó { $rel_segment }: Markdown de talento sin clasificar { $rel }

journal.cli.journal.indexer.original.incomplete = el original del recuerdo está incompleto para { $rel }: { $state }

journal.cli.journal.indexer.original.retained = no se pudo revisar el original del recuerdo para { $rel }; se conservó el índice en caché: { $state }

journal.cli.journal.indexer.original.state.absent = ausente

journal.cli.journal.indexer.original.state.corrupt = corrupto

journal.cli.journal.indexer.original.state.deleted = eliminado

journal.cli.journal.indexer.original.state.ready = listo

journal.cli.journal.indexer.original.state.staged = preparado

journal.cli.journal.indexer.original.state.unavailable = no disponible ({ $detail })

journal.cli.journal.indexer.original.state.unready = no listo

journal.cli.journal.indexer.path.ready = búsqueda de ruta: lista

journal.cli.journal.indexer.path.unready = búsqueda de ruta: no lista

journal.cli.journal.indexer.progress.absent = clasificación: ausente

journal.cli.journal.indexer.progress.complete = clasificación: completa

journal.cli.journal.indexer.progress.incomplete = clasificación: incompleta

journal.cli.journal.indexer.progress.stalled = clasificación: estancada en { $path }

journal.cli.journal.indexer.progress.unknown = clasificación: estancada en una ruta desconocida

journal.cli.journal.indexer.query.column.agent = Agente

journal.cli.journal.indexer.query.column.day = Día

journal.cli.journal.indexer.query.column.facet = Faceta

journal.cli.journal.indexer.query.empty = el índice del diario está vacío: { $path }

journal.cli.journal.indexer.query.error = { $command }: { $detail }

journal.cli.journal.indexer.query.input = falló la lectura de stdin: { $error }

journal.cli.journal.indexer.query.locked = el índice del diario está bloqueado ({ $path }): { $detail }

journal.cli.journal.indexer.query.more = ... +{ $remaining } más

journal.cli.journal.indexer.query.none = No se encontraron resultados

journal.cli.journal.indexer.query.output = falló la escritura de stdout: { $error }

journal.cli.journal.indexer.query.owner = la consulta de conexión necesita al propietario

journal.cli.journal.indexer.query.prompt = buscar>

journal.cli.journal.indexer.query.showing = { $total ->
    [one] Mostrando { $first }-{ $last } de { $total } resultado
    [many] Mostrando { $first }-{ $last } de { $total } resultados
   *[other] Mostrando { $first }-{ $last } de { $total } resultados
    }

journal.cli.journal.indexer.query.unreadable = el índice del diario no se puede leer ({ $path }): { $detail }

journal.cli.journal.indexer.receipt.absent = último escaneo: no registrado

journal.cli.journal.indexer.receipt.full.finished = último escaneo: completo a los { $time } ms, indexados { $indexed }, quitados { $removed }, fallidos { $failed }

journal.cli.journal.indexer.receipt.full.stopped = último escaneo: completo a los { $time } ms, se detuvo: { $error }

journal.cli.journal.indexer.receipt.light.finished = último escaneo: ligero a los { $time } ms, indexados { $indexed }, quitados { $removed }, fallidos { $failed }

journal.cli.journal.indexer.receipt.light.stopped = último escaneo: ligero a los { $time } ms, se detuvo: { $error }

journal.cli.journal.indexer.rel.absolute = { $rel } debe ser journal-{ $rel }ative

journal.cli.journal.indexer.rel.backslash = { $rel } debe usar separadores { $posix }

journal.cli.journal.indexer.rel.empty = { $rel } no debe estar vacío

journal.cli.journal.indexer.rel.invalidcomponent = { $rel } no debe contener componentes vacíos, { $current_component } ni { $parent_component }

journal.cli.journal.indexer.rel.trash = { $rel } no debe nombrar la papelera

journal.cli.journal.indexer.scan.calendar.invites = las invitaciones de calendario todavía no están enlazadas: { $error }

journal.cli.journal.indexer.scan.connections = No se agregaron conexiones nuevas en este reescaneo. Ejecuta { $rebuild_command } para poner al día ahora las que todavía esperan.

journal.cli.journal.indexer.scan.connections.failed = no se revisaron las conexiones contra los cambios de entidades: { $error }

journal.cli.journal.indexer.scan.deferred = un recorrido completo de búsqueda ya está trabajando estos cambios, así que esta revisión se los dejó. ejecuta { $status_command } para ver los detalles.

journal.cli.journal.indexer.scan.edge.skip = Se omite la extracción de aristas para { $rel }: { $error }

journal.cli.journal.indexer.scan.failed = { $indexed ->
    [one] Se indexó { $indexed } archivo, se quitaron { $removed }, se omitieron { $skipped }, fallaron { $failed }
    [many] Se indexaron { $indexed } archivos, se quitaron { $removed }, se omitieron { $skipped }, fallaron { $failed }
   *[other] Se indexaron { $indexed } archivos, se quitaron { $removed }, se omitieron { $skipped }, fallaron { $failed }
    }

journal.cli.journal.indexer.scan.failure = { $failed ->
    [one] { $command }: no se pudo indexar { $failed } actualización; mira las advertencias de arriba
    [many] { $command }: no se pudieron indexar { $failed } actualizaciones; mira las advertencias de arriba
   *[other] { $command }: no se pudieron indexar { $failed } actualizaciones; mira las advertencias de arriba
    }

journal.cli.journal.indexer.scan.invalid.segment = Se omite la extracción de aristas para { $rel }: clave de segmento no válida { $segment }

journal.cli.journal.indexer.scan.mtime = falló la lectura de mtime para { $rel }: { $error }

journal.cli.journal.indexer.scan.retained = { $count ->
    [one] el escaneo ligero conservó { $count } fila desactualizada del día { $day }: el descubrimiento no produjo archivos para ese día; vuelve a ejecutarlo con { $full_flag } para quitarla
    [many] el escaneo ligero conservó { $count } filas desactualizadas del día { $day }: el descubrimiento no produjo archivos para ese día; vuelve a ejecutarlo con { $full_flag } para quitarlas
   *[other] el escaneo ligero conservó { $count } filas desactualizadas del día { $day }: el descubrimiento no produjo archivos para ese día; vuelve a ejecutarlo con { $full_flag } para quitarlas
    }

journal.cli.journal.indexer.scan.summary = { $indexed ->
    [one] Se indexó { $indexed } archivo, se quitaron { $removed }, se omitieron { $skipped }
    [many] Se indexaron { $indexed } archivos, se quitaron { $removed }, se omitieron { $skipped }
   *[other] Se indexaron { $indexed } archivos, se quitaron { $removed }, se omitieron { $skipped }
    }

journal.cli.journal.indexer.speaker.duplicate = transcripción de arista de hablante duplicada { $field } { $sentence } para { $composite_id }

journal.cli.journal.indexer.speaker.missing = falta la transcripción de arista de hablante para { $composite_id }

journal.cli.journal.indexer.speaker.unresolved = transcripción de arista de hablante sin resolver para { $composite_id }

journal.cli.journal.indexer.status.check = última revisión de búsqueda: a los { $time } ms, indexados { $indexed }, quitados { $removed }

journal.cli.journal.indexer.status.check.absent = última revisión de búsqueda: no registrada

journal.cli.journal.indexer.status.check.overdue = última revisión de búsqueda: atrasada, desde entonces se ejecutó sin terminar

journal.cli.journal.indexer.status.failure = fallo: { $reason }

journal.cli.journal.indexer.status.health = estado: { $state } ({ $description })

journal.cli.journal.indexer.status.queued = en cola: { $queued }

journal.cli.journal.indexer.status.removals = elementos por quitar que todavía esperan: { $removals }

journal.cli.journal.indexer.status.wait.days = la más antigua en cola: espera { $days } d { $hours } h

journal.cli.journal.indexer.status.wait.hours = la más antigua en cola: espera { $hours } h { $minutes } min

journal.cli.journal.indexer.status.wait.minutes = la más antigua en cola: espera { $minutes } min

journal.cli.journal.indexer.status.wait.seconds = la más antigua en cola: espera { $seconds } s

journal.cli.journal.indexer.store.admission = falló la admisión del escritor del índice para { $operation }: { $cause }

journal.cli.journal.indexer.store.busy = el escritor del índice está ocupado para { $operation }: { $cause }

journal.cli.journal.indexer.store.encoding = la ruta no es { $encoding } válido: { $path }

journal.cli.journal.indexer.store.generation = el índice de búsqueda lo escribió una versión más nueva de { $brand } (generación del índice { $found }; esta versión lee hasta { $known }), así que esta versión lo deja sin cambios

journal.cli.journal.indexer.store.lookup.cause = { $lookup_command }: { $cause }

journal.cli.journal.indexer.store.missing = no se encontró el archivo: { $path }

journal.cli.journal.indexer.store.original = no se pudo revisar el original del recuerdo en { $path }; se conservó el índice en caché; vuelve a intentarlo cuando la fuente se pueda leer: { $detail }

journal.cli.journal.indexer.store.outside = el archivo está fuera del directorio del diario: { $path }

journal.cli.journal.indexer.stream.json = falló el marcador de flujo { $syntax } para { $path }: { $error }

journal.cli.journal.indexer.stream.path = falló la ruta del marcador de flujo para { $rel_segment }: { $error }

journal.cli.journal.indexer.stream.read = falló la lectura del marcador de flujo para { $path }: { $error }

journal.cli.journal.indexer.warning = advertencia: { $warning }

journal.cli.journal.local.directory = no es un directorio real: { $path }

journal.cli.journal.local.failure = { $invocation } { $token }: { $message }

journal.cli.journal.local.resolution.config = falló la resolución del diario: falló la decodificación de la configuración

journal.cli.journal.local.resolution.home = falló la resolución del diario: el directorio de inicio no está disponible

journal.cli.journal.local.unknown = operación local desconocida

journal.cli.journal.notify.error.facet = { $invocation }: error: { $option } ya no es compatible. Pon la selección de facetas en { $action_flag }; por ejemplo, { $action_flag } { $example }.

journal.cli.journal.notify.error.integer = { $invocation }: error: argumento { $auto_dismiss_flag }: valor entero no válido: '{ $value }'

journal.cli.journal.notify.error.message = { $invocation }: error: los siguientes argumentos son obligatorios: { $positional }

journal.cli.journal.notify.error.unknown = { $invocation }: error: argumentos no reconocidos: { $token }

journal.cli.journal.notify.error.value = { $invocation }: error: argumento { $option }: se esperaba un argumento

journal.cli.journal.notify.failed = No se pudo enviar la notificación (¿está { $channel } en ejecución?)

journal.cli.journal.notify.help = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }                  { $message_options }{ $line_break }{ $line_break }Enviar una notificación por medio de { $channel }{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $positional }texto del mensaje de la notificación{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }mostrar este mensaje de ayuda y salir{ $line_break }{ $indent_2 }{ $title_option }{ $gap_9 }título de la notificación{ $line_break }{ $indent_2 }{ $icon_option }{ $gap_11 }nombre de ícono de { $icon_set } (predeterminado: { $mailbox }){ $line_break }{ $indent_2 }{ $event_option }{ $gap_9 }nombre del evento (predeterminado: { $default_show }{ $line_break }{ $indent_2 }{ $action_option }{ $gap_7 }ruta URL que se abre al hacer clic{ $line_break }{ $indent_2 }{ $app_option }{ $gap_13 }nombre de la app de origen{ $line_break }{ $indent_2 }{ $badge_option }{ $gap_9 }texto o número de la insignia{ $line_break }{ $syntax_continuation_18 }{ $line_break }                        descartar automáticamente después de N milisegundos{ $line_break }{ $indent_2 }{ $no_dismiss_option }{ $gap_10 }hacer que la notificación no se pueda descartar{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_9 }Activar la salida detallada{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_11 }Activar el registro de depuración

journal.cli.journal.notify.sent = Notificación enviada

journal.cli.journal.notify.utf8 = falló la notificación nativa del diario: los argumentos del propietario no son UTF-8 válido

journal.cli.journal.process.admission = falló la admisión del proceso del diario: { $detail }

journal.cli.journal.process.body = falló el inicio del proceso nativo del diario: no hay cuerpo nativo para { $token }

journal.cli.journal.process.launch = falló el inicio del proceso nativo del diario: { $error }

journal.cli.journal.resolution.config = falló la resolución nativa del diario: falló la decodificación de la configuración

journal.cli.journal.resolution.home = falló la resolución nativa del diario: el directorio de inicio no está disponible

journal.cli.journal.root.inspect = falló la resolución nativa de la raíz del proyecto del diario: no se pudo inspeccionar el ejecutable actual: { $error }

journal.cli.journal.root.package = falló la resolución nativa de la raíz del proyecto del diario: no se pudo encontrar la copia de trabajo de origen ni el paquete { $product } instalado desde { $path }

journal.cli.sol.call.activities.day.invalid = Error: día no válido '{ $value }'

journal.cli.sol.call.activities.day.missing = Error: el día es obligatorio (pásalo como argumento o define { $literal_1 }).

journal.cli.sol.call.activities.empty = No se encontraron actividades.

journal.cli.sol.call.activities.exists = Error: la actividad ya existe: { $detail }

journal.cli.sol.call.activities.facet.missing = Error: la faceta es obligatoria (pásala como argumento o define { $literal_1 }).

journal.cli.sol.call.activities.facet.unknown = Error: actividad desconocida para la faceta '{ $facet }': { $activity_type }

journal.cli.sol.call.activities.kind.option = Error: { $literal_1 } es obligatorio.

journal.cli.sol.call.activities.kind.required = Error: la actividad es obligatoria.

journal.cli.sol.call.activities.participation.array = Error: { $literal_1 } debe ser un arreglo

journal.cli.sol.call.activities.participation.confidence = Error: { $literal_1 }[{ $index }] '{ $literal_2 }' debe ser un número

journal.cli.sol.call.activities.participation.context = Error: { $literal_1 }[{ $index }] '{ $literal_2 }' debe ser una cadena

journal.cli.sol.call.activities.participation.name = Error: { $literal_1 }[{ $index }] requiere una cadena no vacía '{ $literal_2 }'

journal.cli.sol.call.activities.participation.object = Error: { $literal_1 }[{ $index }] debe ser un objeto

journal.cli.sol.call.activities.participation.role = Error: { $literal_1 }[{ $index }] tiene un rol no válido '{ $role }' (debe ser uno de ['{ $literal_2 }', '{ $literal_3 }', '{ $literal_4 }', '{ $literal_5 }'])

journal.cli.sol.call.activities.participation.source = Error: { $literal_1 }[{ $index }] tiene una fuente no válida '{ $source }' (debe ser uno de ['{ $literal_2 }', '{ $literal_3 }', '{ $literal_4 }', '{ $literal_5 }', '{ $literal_6 }'])

journal.cli.sol.call.activities.patch.disallowed = Error: campos de actualización no permitidos: { $fields }

journal.cli.sol.call.activities.patch.empty = Error: la carga de la actualización debe incluir al menos un campo modificable.

journal.cli.sol.call.activities.range.options = Error: { $literal_1 } no es compatible con { $literal_2 }/{ $literal_3 }.

journal.cli.sol.call.activities.range.reversed = Error: { $literal_1 } ({ $end_day }) no debe ser anterior a { $literal_2 } ({ $start_day })

journal.cli.sol.call.activities.segment.invalid = Error: { $literal_1 } '{ $segment }' no es válido (se esperaba { $literal_2 })

journal.cli.sol.call.activities.source.create = Error: { $literal_1 } debe ser '{ $literal_2 }'.

journal.cli.sol.call.activities.source.list = Error: { $literal_1 } debe ser '{ $literal_2 }' o '{ $literal_3 }'.

journal.cli.sol.call.activities.span.absent = no se encontró la actividad: { $span_id }

journal.cli.sol.call.activities.span.missing = Error: falta el argumento '{ $literal_1 }'.

journal.cli.sol.call.activities.stdin.json = Error: { $literal_1 } no válido en stdin: { $error }

journal.cli.sol.call.activities.stdin.object = Error: se esperaba un objeto { $literal_1 } en stdin.

journal.cli.sol.call.activities.title.option = Error: { $literal_1 } es obligatorio.

journal.cli.sol.call.activities.title.required = Error: el título es obligatorio.

journal.cli.sol.call.body.coverage = cobertura: { $start } a { $end }

journal.cli.sol.call.body.day = día: { $day }

journal.cli.sol.call.body.days = días: { $days }

journal.cli.sol.call.body.entries = entradas: { $count }

journal.cli.sol.call.body.imports = importaciones: { $imported }

journal.cli.sol.call.body.range = ventana: { $start } a { $end }

journal.cli.sol.call.body.sources = fuentes: { $sources }

journal.cli.sol.call.body.unreadable = no se pudo leer la respuesta de la app de cuerpo.

journal.cli.sol.call.facets.accepted = Se aceptó el candidato de faceta '{ $name_key }' como '{ $facet }'.

journal.cli.sol.call.facets.action = resultado de { $action } para '{ $name_key }': { $result }

journal.cli.sol.call.facets.already.accepted = El candidato de faceta '{ $name_key }' ya estaba aceptado.

journal.cli.sol.call.facets.already.dismissed = El candidato de faceta '{ $name_key }' ya estaba descartado.

journal.cli.sol.call.facets.candidate = { $name }  [{ $status }]  count={ $count }  last={ $day }

journal.cli.sol.call.facets.dismissed = Se descartó el candidato de faceta '{ $name_key }'.

journal.cli.sol.call.facets.empty = No se encontraron candidatos de faceta.

journal.cli.sol.call.thinking.attestation.expires = { $field_key }: { $value }

journal.cli.sol.call.thinking.attestation.observed = { $field_key }: { $value }

journal.cli.sol.call.thinking.attestation.reason = { $field_key }: { $value }

journal.cli.sol.call.thinking.attestation.state = { $field_key }: { $value }

journal.cli.sol.call.thinking.continuing = la operación sigue del lado del servidor; { $literal_1 } muestra su avance.

journal.cli.sol.call.thinking.enabled = { $field_key }: { $value }

journal.cli.sol.call.thinking.ended = la operación terminó sin activar el procesamiento confidencial; revisa { $literal_1 }.

journal.cli.sol.call.thinking.lane = carril activo: { $lane }

journal.cli.sol.call.thinking.next = siguiente: { $literal_1 }

journal.cli.sol.call.thinking.not.ready = no listo

journal.cli.sol.call.thinking.operation = operación: { $operation_phase }

journal.cli.sol.call.thinking.portal = continúa en el navegador { $arrow } { $url }

journal.cli.sol.call.thinking.provenance = { $field_key }: { $value }

journal.cli.sol.call.thinking.ready = listo

journal.cli.sol.call.thinking.subscribe = { $field_key }: { $url }

journal.cli.sol.call.transcripts.actionable = { $marker } accionable: { $literal_1 }

journal.cli.sol.call.transcripts.day.totals = { $day }  transcripciones:{ $n_transcripts } perceptos:{ $n_percepts }

journal.cli.sol.call.transcripts.full.raw = Error: No se puede usar { $literal_1 } y { $literal_2 } juntos.

journal.cli.sol.call.transcripts.heading.percepts = Perceptos:

journal.cli.sol.call.transcripts.heading.transcripts = Transcripciones:

journal.cli.sol.call.transcripts.length.integer = Error: la opción { $literal_1 } requiere un entero.

journal.cli.sol.call.transcripts.max.integer = Error: la opción { $literal_1 } requiere un entero.

journal.cli.sol.call.transcripts.month.empty = No hay datos de { $month }.

journal.cli.sol.call.transcripts.none = (ninguno)

journal.cli.sol.call.transcripts.pending = { $count ->
    [one] { $indent }{ $start } - { $end } ({ $display_count } segmento pendiente en { $starts })
    [many] { $indent }{ $start } - { $end } ({ $display_count } segmentos pendientes en { $starts })
   *[other] { $indent }{ $start } - { $end } ({ $display_count } segmentos pendientes en { $starts })
    }

journal.cli.sol.call.transcripts.range.flags = Error: No se puede mezclar { $literal_1 }, { $literal_2 } y { $literal_3 }/{ $literal_4 }.

journal.cli.sol.call.transcripts.segments.empty = No hay segmentos.

journal.cli.sol.call.transcripts.source.flags = Error: No se puede mezclar { $literal_1 }/{ $literal_2 } con las opciones de fuente individuales.

journal.cli.sol.call.transcripts.speakers.heading = Hablantes de { $day }/{ $stream }/{ $segment }:

journal.cli.sol.call.transcripts.start.invalid = Error: la hora de inicio debe ser { $literal_1 }.

journal.cli.sol.call.transcripts.tag.owner = { $marker } no accionable: { $literal_1 }

journal.cli.sol.call.transcripts.total.days = { $count ->
    [one] Total: { $display_count } día con datos
    [many] Total: { $display_count } días con datos
   *[other] Total: { $display_count } días con datos
    }

journal.cli.sol.call.transcripts.truncated = [truncado: { $total } bytes en total, { $literal_1 } { $maximum }]

journal.cli.sol.call.transcripts.unknown.voice = voz desconocida

journal.cli.sol.call.typer.error = Error

journal.cli.sol.call.typer.missing.argument = Falta el argumento '{ $name }'.

journal.cli.sol.call.typer.missing.option = Falta la opción '{ $option }'.

journal.cli.sol.call.typer.try = Prueba '{ $command } { $literal_1 }' para ver la ayuda.

journal.cli.sol.entities.accept.merge.candidate.accept.result.for = resultado de aceptación para { $source_slug } -> { $target_slug }: { $status }

journal.cli.sol.entities.accept.merge.candidate.accepted.merge.candidate = Candidato de fusión aceptado: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.accept.merge.candidate.merge.candidate.already.accepted = El candidato de fusión ya estaba aceptado: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.accept.merge.candidate.merge.is.permanent = La fusión { $merge_id } es permanente.

journal.cli.sol.entities.aka.added.alias.to = Se agregó el alias '{ $aka_value }' a '{ $resolved_name }'.

journal.cli.sol.entities.aka.alias.already.exists.for = El alias '{ $aka_value }' ya existe para '{ $resolved_name }'.

journal.cli.sol.entities.aka.alias.is.the.first.word = El alias '{ $aka_value }' es la primera palabra de '{ $resolved_name }' (omitido).

journal.cli.sol.entities.ambiguities.error.status.must.be.open = Error: { $option_status } debe ser open o resolved.

journal.cli.sol.entities.ambiguities.from = { $spacing_2 }de: { $sources }

journal.cli.sol.entities.ambiguities.no.entity.ambiguities.found = No se encontraron ambigüedades de entidad.

journal.cli.sol.entities.ambiguities.scope.tier = { $ambiguity_id }{ $spacing_2 }{ $name }{ $spacing_2 }[{ $status }]{ $spacing_2 }scope={ $scope }{ $spacing_2 }tier={ $tier }

journal.cli.sol.entities.ambiguities.score = { $spacing_2 }- { $candidate } score={ $score }

journal.cli.sol.entities.attach.entity.already.attached = La entidad '{ $entity }' ya está asociada.

journal.cli.sol.entities.attach.entity.attached = Entidad '{ $entity }' asociada.

journal.cli.sol.entities.busy = esa entidad no se pudo actualizar ahora porque estaba ocupada. vuelve a intentarlo dentro de un momento.

journal.cli.sol.entities.delete.approval = Eliminar mueve la entidad y sus enlaces de faceta a la papelera; { $option_erase } la borra para siempre. Cuando el propietario lo apruebe, vuelve a ejecutarlo con { $option_yes }.

journal.cli.sol.entities.delete.entity.erased = Se borró '{ $entity_id }'.

journal.cli.sol.entities.delete.entity.error.wasn.t.deleted = Error: no se eliminó '{ $entity_id }': { $reason }.

journal.cli.sol.entities.delete.failed.without.reason.error.wasn.t.deleted.it = Error: no se eliminó '{ $entity_id }': no se eliminó.

journal.cli.sol.entities.detect.entity.detected.for = Entidad '{ $entity_name }' detectada para { $day }.

journal.cli.sol.entities.dismiss.merge.candidate.dismiss.result.for = resultado de descarte para { $source_slug } -> { $target_slug }: { $status }

journal.cli.sol.entities.dismiss.merge.candidate.dismissed.merge.candidate = Candidato de fusión descartado: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.dismiss.merge.candidate.merge.candidate.already.dismissed = El candidato de fusión ya estaba descartado: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.edge.resolution.error.error.entity.not.found = Error: no se encontró la entidad '{ $query }'.

journal.cli.sol.entities.edge.resolution.error.error.entity.not.found.did = Error: no se encontró la entidad '{ $query }'. ¿Quisiste decir: { $labels }?

journal.cli.sol.entities.edge.resolution.without.query.empty.error.entity.entity.not.found = Error: no se encontró la entidad 'entity'.

journal.cli.sol.entities.edge.resolution.without.query.suggestions.error.entity.entity.not.found = Error: no se encontró la entidad 'entity'. ¿Quisiste decir: { $labels }?

journal.cli.sol.entities.edges.unavailable = Las conexiones no están disponibles en este momento.

journal.cli.sol.entities.entity.error.error.entity.is.hidden = Error: la entidad '{ $name }' está oculta.

journal.cli.sol.entities.entity.error.error.entity.not.found = Error: no se encontró la entidad '{ $name }'.

journal.cli.sol.entities.entity.error.error.invalid.entity.type = Error: tipo de entidad no válido '{ $entity_type }'.

journal.cli.sol.entities.entity.versions.merge = { $spacing_2 }merge={ $merge_id } ({ $description })

journal.cli.sol.entities.entity.versions.no.identity.history.for = No hay historial de identidad para { $entity }.

journal.cli.sol.entities.entity.versions.unknown.no.identity.history.for.entity = No hay historial de identidad para entity.

journal.cli.sol.entities.error.without.name.hidden.error.entity.entity.is.hidden = Error: la entidad 'entity' está oculta.

journal.cli.sol.entities.evidence.default.day.unknown.day = unknown-day

journal.cli.sol.entities.evidence.kind.missing.unknown = unknown

journal.cli.sol.entities.history.all.evidence.rows.for = { $quantity ->
    [one] { $total } fila de evidencia para { $entity_id } <-> { $peer }:
    [many] { $total } filas de evidencia para { $entity_id } <-> { $peer }:
   *[other] { $total } filas de evidencia para { $entity_id } <-> { $peer }:
    }

journal.cli.sol.entities.history.no.recorded.connection.history.between = No hay historial de conexión registrado entre { $entity_id } y { $peer }.

journal.cli.sol.entities.history.no.visible.rows = { $quantity ->
    [one] { $total } fila de evidencia para { $entity_id } <-> { $peer } (mostrando 0):
    [many] { $total } filas de evidencia para { $entity_id } <-> { $peer } (mostrando 0):
   *[other] { $total } filas de evidencia para { $entity_id } <-> { $peer } (mostrando 0):
    }

journal.cli.sol.entities.history.range.evidence.rows.for.showing = { $quantity ->
    [one] { $total } fila de evidencia para { $entity_id } <-> { $peer } (mostrando { $first }-{ $last }):
    [many] { $total } filas de evidencia para { $entity_id } <-> { $peer } (mostrando { $first }-{ $last }):
   *[other] { $total } filas de evidencia para { $entity_id } <-> { $peer } (mostrando { $first }-{ $last }):
    }

journal.cli.sol.entities.list.attached.attached.entities = { $quantity ->
    [one] { $count } entidad asociada:
    [many] { $count } entidades asociadas:
   *[other] { $count } entidades asociadas:
    }

journal.cli.sol.entities.list.detected.detected.for.entities = { $quantity ->
    [one] { $count } entidad detectada para { $day }:
    [many] { $count } detectadas para { $day } entidades:
   *[other] { $count } detectadas para { $day } entidades:
    }

journal.cli.sol.entities.list.hidden.hidden = { $spacing_2 }- { $name } oculta ({ $entity_type }): { $description }

journal.cli.sol.entities.list.no.entities.found = No se encontraron entidades.

journal.cli.sol.entities.list.visible.row = { $spacing_2 }- { $name } ({ $entity_type }): { $description }

journal.cli.sol.entities.merge.approval = Una fusión no se puede deshacer. Previsualízala sin { $option_commit } y, cuando el propietario lo apruebe, vuelve a ejecutarla con { $option_commit } { $option_yes }.

journal.cli.sol.entities.merge.candidates.facet.detections.needs.last = { $source } -> { $target }{ $spacing_2 }[{ $status }]{ $spacing_2 }facet={ $facet }{ $spacing_2 }detections={ $detections }{ $spacing_2 }needs={ $needs }{ $spacing_2 }last={ $last }

journal.cli.sol.entities.merge.candidates.no.merge.candidates.found = No se encontraron candidatos de fusión.

journal.cli.sol.entities.merge.failure.default.error.operation.failed = Error: la operación falló

journal.cli.sol.entities.merge.preview.aliases.added = { $spacing_2 }alias agregados: { $aliases }

journal.cli.sol.entities.merge.preview.emails.added = { $spacing_2 }correos agregados: { $emails }

journal.cli.sol.entities.merge.preview.facet.links.moved.merged = { $spacing_2 }enlaces de faceta: { $moved } movidos, { $merged } fusionados

journal.cli.sol.entities.merge.preview.merge.preview = Vista previa de la fusión:

journal.cli.sol.entities.merge.preview.observations.moved = { $spacing_2 }observaciones movidas: { $observations }

journal.cli.sol.entities.merge.preview.segment.update.errors = { $spacing_2 }errores de actualización de segmentos: { $errors }

journal.cli.sol.entities.merge.preview.speaker.labels.updated.labels.corrections = { $quantity ->
    [one] { $corrections_quantity ->
    [one] { $spacing_2 }etiquetas de hablante actualizadas: { $labels } etiqueta, { $corrections } corrección
    [many] { $spacing_2 }etiquetas de hablante actualizadas: { $labels } etiqueta, { $corrections } correcciones
   *[other] { $spacing_2 }etiquetas de hablante actualizadas: { $labels } etiqueta, { $corrections } correcciones
    }
    [many] { $corrections_quantity ->
    [one] { $spacing_2 }etiquetas de hablante actualizadas: { $labels } etiquetas, { $corrections } corrección
    [many] { $spacing_2 }etiquetas de hablante actualizadas: { $labels } etiquetas, { $corrections } correcciones
   *[other] { $spacing_2 }etiquetas de hablante actualizadas: { $labels } etiquetas, { $corrections } correcciones
    }
   *[other] { $corrections_quantity ->
    [one] { $spacing_2 }etiquetas de hablante actualizadas: { $labels } etiquetas, { $corrections } corrección
    [many] { $spacing_2 }etiquetas de hablante actualizadas: { $labels } etiquetas, { $corrections } correcciones
   *[other] { $spacing_2 }etiquetas de hablante actualizadas: { $labels } etiquetas, { $corrections } correcciones
    }
    }

journal.cli.sol.entities.merge.preview.this.preview.covers.alias.and = { $spacing_2 }Esta vista previa cubre las adiciones de alias y de correos; la fusión también puede cambiar enlaces de faceta, notas, etiquetas de hablante y muestras de voz.

journal.cli.sol.entities.merge.preview.voice.samples.moved.added.total = { $spacing_2 }muestras de voz movidas: { $added } agregadas, { $total } en total

journal.cli.sol.entities.move.entity.error.facet.from.does.not = Error: la faceta '{ $from_facet }' ({ $option_from }) no existe.

journal.cli.sol.entities.move.entity.error.facet.to.does.not = Error: la faceta '{ $to_facet }' ({ $option_to }) no existe.

journal.cli.sol.entities.move.entity.moved.entity.from.to = Se movió la entidad '{ $entity_name }' de '{ $from_facet }' a '{ $to_facet }'.

journal.cli.sol.entities.neighbor.count = count={ $count }

journal.cli.sol.entities.neighbor.kinds = kinds={ $kinds }

journal.cli.sol.entities.neighbor.score = score={ $score }

journal.cli.sol.entities.network.all.recorded.connections.for = { $quantity ->
    [one] { $total } conexión registrada para { $entity_id }:
    [many] { $total } conexiones registradas para { $entity_id }:
   *[other] { $total } conexiones registradas para { $entity_id }:
    }

journal.cli.sol.entities.network.directed.both.directed.out.in = directed=out:{ $out_count }, in:{ $in_count }

journal.cli.sol.entities.network.directed.in.directed.in = directed=in:{ $in_count }

journal.cli.sol.entities.network.directed.out.directed.out = directed=out:{ $out_count }

journal.cli.sol.entities.network.limited.recorded.connections.for.showing = { $quantity ->
    [one] { $total } conexión registrada para { $entity_id } (mostrando { $shown }):
    [many] { $total } conexiones registradas para { $entity_id } (mostrando { $shown }):
   *[other] { $total } conexiones registradas para { $entity_id } (mostrando { $shown }):
    }

journal.cli.sol.entities.network.no.recorded.connections.for = No hay conexiones registradas para { $entity_id }.

journal.cli.sol.entities.network.seen.range.seen = seen={ $first }..{ $last }

journal.cli.sol.entities.network.seen.seen = seen={ $day }

journal.cli.sol.entities.observation.error.couldn.t.read.the.moments = no se pudieron leer los momentos guardados para '{ $name }': la línea { $line_str } del archivo no es válida. no se cambió nada.

journal.cli.sol.entities.observations.newest.moments.for.in.newest.first = { $quantity ->
    [one] { $total } momento para '{ $name }' en '{ $facet }', el más reciente primero:
    [many] { $total } momentos para '{ $name }' en '{ $facet }', el más reciente primero:
   *[other] { $total } momentos para '{ $name }' en '{ $facet }', el más reciente primero:
    }

journal.cli.sol.entities.observations.nothing.with.yet = nada con { $resolved_name } todavía.

journal.cli.sol.entities.observations.oldest.moments.for.in.oldest.first = { $quantity ->
    [one] { $total } momento para '{ $name }' en '{ $facet }', el más antiguo primero:
    [many] { $total } momentos para '{ $name }' en '{ $facet }', el más antiguo primero:
   *[other] { $total } momentos para '{ $name }' en '{ $facet }', el más antiguo primero:
    }

journal.cli.sol.entities.observations.partial.most.recent.showing.the.most.recent.of = se muestran los { $shown } más recientes de { $total } momentos para '{ $name }'. agrega { $option_all } para verlos todos.

journal.cli.sol.entities.observations.partial.oldest.showing.the.oldest.of.moments = se muestran los { $shown } más antiguos de { $total } momentos para '{ $name }'. agrega { $option_all } para verlos todos.

journal.cli.sol.entities.observe.added.added.moments.for.in = { $quantity ->
    [one] agregado. { $count } momento para '{ $name }' en '{ $facet }'.
    [many] agregado. { $count } momentos para '{ $name }' en '{ $facet }'.
   *[other] agregado. { $count } momentos para '{ $name }' en '{ $facet }'.
    }

journal.cli.sol.entities.observe.your.journal.already.holds.that = tu diario ya guarda ese momento para '{ $resolved_name }'. no se agregó nada.

journal.cli.sol.entities.overview.all.network.overview.edges.across.entities = { $quantity ->
    [one] { $entities_quantity ->
    [one] Vista de la red: { $edges } arista en { $entities } entidad:
    [many] Vista de la red: { $edges } arista en { $entities } entidades:
   *[other] Vista de la red: { $edges } arista en { $entities } entidades:
    }
    [many] { $entities_quantity ->
    [one] Vista de la red: { $edges } aristas en { $entities } entidad:
    [many] Vista de la red: { $edges } aristas en { $entities } entidades:
   *[other] Vista de la red: { $edges } aristas en { $entities } entidades:
    }
   *[other] { $entities_quantity ->
    [one] Vista de la red: { $edges } aristas en { $entities } entidad:
    [many] Vista de la red: { $edges } aristas en { $entities } entidades:
   *[other] Vista de la red: { $edges } aristas en { $entities } entidades:
    }
    }

journal.cli.sol.entities.overview.kinds = Tipos: { $kinds }

journal.cli.sol.entities.overview.limited.network.overview.edges.across.entities = { $quantity ->
    [one] { $entities_quantity ->
    [one] Vista de la red: { $edges } arista en { $entities } entidad (mostrando { $shown }):
    [many] Vista de la red: { $edges } arista en { $entities } entidades (mostrando { $shown }):
   *[other] Vista de la red: { $edges } arista en { $entities } entidades (mostrando { $shown }):
    }
    [many] { $entities_quantity ->
    [one] Vista de la red: { $edges } aristas en { $entities } entidad (mostrando { $shown }):
    [many] Vista de la red: { $edges } aristas en { $entities } entidades (mostrando { $shown }):
   *[other] Vista de la red: { $edges } aristas en { $entities } entidades (mostrando { $shown }):
    }
   *[other] { $entities_quantity ->
    [one] Vista de la red: { $edges } aristas en { $entities } entidad (mostrando { $shown }):
    [many] Vista de la red: { $edges } aristas en { $entities } entidades (mostrando { $shown }):
   *[other] Vista de la red: { $edges } aristas en { $entities } entidades (mostrando { $shown }):
    }
    }

journal.cli.sol.entities.overview.no.recorded.connections.in.the = No hay conexiones registradas en el índice de aristas.

journal.cli.sol.entities.overview.type = type={ $entity_type }

journal.cli.sol.entities.record.merge.candidate.merge.candidate.recorded = candidato de fusión registrado: { $source } -> { $target }

journal.cli.sol.entities.record.merge.candidate.merge.candidate.updated.status = candidato de fusión actualizado: { $source } -> { $target } (estado: { $status })

journal.cli.sol.entities.resolve.ambiguity.group.error.missing.required.option.member = Error: falta la opción obligatoria { $option_member_id }

journal.cli.sol.entities.resolve.ambiguity.group.error.missing.required.option.revision = Error: falta la opción obligatoria { $option_revision }

journal.cli.sol.entities.resolve.ambiguity.group.refusing.to.resolve.this.ambiguity = No se resuelve este grupo de ambigüedades sin { $option_yes }.

journal.cli.sol.entities.resolve.ambiguity.group.resolved.ambiguities.to = { $quantity ->
    [one] Se resolvió { $count } ambigüedad hacia { $entity_id }.
    [many] Se resolvieron { $count } ambigüedades hacia { $entity_id }.
   *[other] Se resolvieron { $count } ambigüedades hacia { $entity_id }.
    }

journal.cli.sol.entities.resolve.ambiguity.refusing.to.resolve.this.ambiguity = No se resuelve esta ambigüedad sin { $option_yes }.

journal.cli.sol.entities.resolve.ambiguity.resolved.to.at = Se resolvió { $ambiguity_id } hacia { $entity_id } en { $resolved_at }.

journal.cli.sol.entities.resolve.day.error.day.is.required.pass = Error: el día es obligatorio (pásalo como argumento o define { $literal_sol_day }).

journal.cli.sol.entities.resolve.error.error.entity.is.hidden = Error: la entidad '{ $blocked_name }' está oculta.

journal.cli.sol.entities.resolve.error.error.entity.not.found.did = Error: no se encontró la entidad '{ $entity }'. ¿Quisiste decir: { $names }?

journal.cli.sol.entities.resolve.error.error.entity.not.found.in = Error: no se encontró la entidad '{ $entity }' en la faceta '{ $facet }'.

journal.cli.sol.entities.resolve.facet.error.facet.is.required.pass = Error: la faceta es obligatoria (pásala como argumento o define { $literal_sol_facet }).

journal.cli.sol.entities.restore.review.error.missing.required.option.ambiguity = Error: falta la opción obligatoria { $option_ambiguity_id }

journal.cli.sol.entities.restore.review.error.missing.required.option.facet = Error: falta la opción obligatoria { $option_facet }

journal.cli.sol.entities.restore.review.error.missing.required.option.source = Error: falta la opción obligatoria { $option_source_slug }

journal.cli.sol.entities.restore.review.error.missing.required.option.target = Error: falta la opción obligatoria { $option_target_slug }

journal.cli.sol.entities.restore.review.error.target.must.be.ambiguity = Error: { $literal_target } debe ser ambiguity o merge_candidate

journal.cli.sol.entities.restore.review.refusing.to.restore.review.item = No se restaura el elemento de revisión sin { $option_yes }.

journal.cli.sol.entities.restore.review.restored.review.item = Se restauró el elemento de revisión { $target }.

journal.cli.sol.entities.restore.version.refusing.to.restore.this.identity = No se restaura esta versión de identidad sin { $option_yes }.

journal.cli.sol.entities.restore.version.restored.from.new.history.version = Se restauró { $entity_id } desde { $version_id }; nueva versión del historial { $version }.

journal.cli.sol.entities.review.review.items.ambiguity.card.s = { $quantity ->
    [one] { $ambiguities_quantity ->
    [one] { $candidates_quantity ->
    [one] Elementos de revisión: { $cards } tarjeta de ambigüedad, { $ambiguities } ambigüedad apartada, { $candidates } candidato de fusión apartado.
    [many] Elementos de revisión: { $cards } tarjeta de ambigüedad, { $ambiguities } ambigüedad apartada, { $candidates } candidatos de fusión apartados.
   *[other] Elementos de revisión: { $cards } tarjeta de ambigüedad, { $ambiguities } ambigüedad apartada, { $candidates } candidatos de fusión apartados.
    }
    [many] { $candidates_quantity ->
    [one] Elementos de revisión: { $cards } tarjeta de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidato de fusión apartado.
    [many] Elementos de revisión: { $cards } tarjeta de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidatos de fusión apartados.
   *[other] Elementos de revisión: { $cards } tarjeta de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidatos de fusión apartados.
    }
   *[other] { $candidates_quantity ->
    [one] Elementos de revisión: { $cards } tarjeta de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidato de fusión apartado.
    [many] Elementos de revisión: { $cards } tarjeta de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidatos de fusión apartados.
   *[other] Elementos de revisión: { $cards } tarjeta de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidatos de fusión apartados.
    }
    }
    [many] { $ambiguities_quantity ->
    [one] { $candidates_quantity ->
    [one] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedad apartada, { $candidates } candidato de fusión apartado.
    [many] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedad apartada, { $candidates } candidatos de fusión apartados.
   *[other] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedad apartada, { $candidates } candidatos de fusión apartados.
    }
    [many] { $candidates_quantity ->
    [one] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidato de fusión apartado.
    [many] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidatos de fusión apartados.
   *[other] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidatos de fusión apartados.
    }
   *[other] { $candidates_quantity ->
    [one] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidato de fusión apartado.
    [many] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidatos de fusión apartados.
   *[other] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidatos de fusión apartados.
    }
    }
   *[other] { $ambiguities_quantity ->
    [one] { $candidates_quantity ->
    [one] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedad apartada, { $candidates } candidato de fusión apartado.
    [many] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedad apartada, { $candidates } candidatos de fusión apartados.
   *[other] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedad apartada, { $candidates } candidatos de fusión apartados.
    }
    [many] { $candidates_quantity ->
    [one] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidato de fusión apartado.
    [many] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidatos de fusión apartados.
   *[other] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidatos de fusión apartados.
    }
   *[other] { $candidates_quantity ->
    [one] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidato de fusión apartado.
    [many] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidatos de fusión apartados.
   *[other] Elementos de revisión: { $cards } tarjetas de ambigüedad, { $ambiguities } ambigüedades apartadas, { $candidates } candidatos de fusión apartados.
    }
    }
    }

journal.cli.sol.entities.search.activity.unreadable = La actividad de entidades detectada no se puede leer. Ejecuta `{ $command }`, repara el registro indicado y vuelve a intentarlo.

journal.cli.sol.entities.search.entities = { $quantity ->
    [one] { $count } entidad:
    [many] { $count } entidades:
   *[other] { $count } entidades:
    }

journal.cli.sol.entities.search.facets = { $spacing_4 }facetas: { $facets }

journal.cli.sol.entities.search.index.busy = La búsqueda de entidades no está disponible mientras la indexación está en curso. Vuelve a intentarlo cuando termine.

journal.cli.sol.entities.search.index.stale = El índice de búsqueda de entidades está desactualizado. Ejecuta `{ $command }` y vuelve a intentarlo.

journal.cli.sol.entities.search.index.unavailable = El índice de búsqueda de entidades no está disponible. Ejecuta `{ $command }` y vuelve a intentarlo.

journal.cli.sol.entities.sweep.review.completed.entity.review.sweep = Barrido de revisión de entidades completado.

journal.cli.sol.entities.sweep.review.refusing.to.run.review.sweep = No se ejecuta el barrido de revisión sin { $option_yes }.

journal.cli.sol.entities.update.entity.updated = Entidad '{ $resolved_name }' actualizada.

journal.cli.sol.entities.update.entity.updated.for = Entidad '{ $entity }' actualizada para { $day }.

journal.cli.sol.health.activities.count = { $space_2 }{ $activities_count }: { $value }

journal.cli.sol.health.activities.user.edited = { $space_2 }{ $activities_user_edited }: { $value }

journal.cli.sol.health.activities.with.participation = { $space_2 }{ $activities_with_participation }: { $value }

journal.cli.sol.health.activities.with.story = { $space_2 }{ $activities_with_story }: { $value }

journal.cli.sol.health.argument.unexpected = Error: argumento inesperado { $token }.

journal.cli.sol.health.backlog.days = { $days_quantity ->
    [one] { $space_3 }a lo largo de { $days } día
    [many] { $space_3 }a lo largo de { $days } días
   *[other] { $space_3 }a lo largo de { $days } días
    }

journal.cli.sol.health.backlog.days.incomplete = { $days_quantity ->
    [one] { $space_3 }a lo largo de { $days } día (estado incompleto)
    [many] { $space_3 }a lo largo de { $days } días (estado incompleto)
   *[other] { $space_3 }a lo largo de { $days } días (estado incompleto)
    }

journal.cli.sol.health.backlog.media.days = { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }{ $media } segmento en espera de procesamiento de medios a lo largo de { $days } día
    [many] { $space_2 }{ $media } segmento en espera de procesamiento de medios a lo largo de { $days } días
   *[other] { $space_2 }{ $media } segmento en espera de procesamiento de medios a lo largo de { $days } días
    }
    [many] { $days_quantity ->
    [one] { $space_2 }{ $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día
    [many] { $space_2 }{ $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
   *[other] { $space_2 }{ $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }{ $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día
    [many] { $space_2 }{ $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
   *[other] { $space_2 }{ $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
    }
    }

journal.cli.sol.health.backlog.media.days.incomplete = { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }al menos { $media } segmento en espera de procesamiento de medios a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
    }
    [many] { $days_quantity ->
    [one] { $space_2 }al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
    }
    }

journal.cli.sol.health.backlog.thinking.days = { $thinking_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segmento en espera de razonamiento a lo largo de { $days } día
    [many] { $space_2 }{ $thinking } segmento en espera de razonamiento a lo largo de { $days } días
   *[other] { $space_2 }{ $thinking } segmento en espera de razonamiento a lo largo de { $days } días
    }
    [many] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segmentos en espera de razonamiento a lo largo de { $days } día
    [many] { $space_2 }{ $thinking } segmentos en espera de razonamiento a lo largo de { $days } días
   *[other] { $space_2 }{ $thinking } segmentos en espera de razonamiento a lo largo de { $days } días
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segmentos en espera de razonamiento a lo largo de { $days } día
    [many] { $space_2 }{ $thinking } segmentos en espera de razonamiento a lo largo de { $days } días
   *[other] { $space_2 }{ $thinking } segmentos en espera de razonamiento a lo largo de { $days } días
    }
    }

journal.cli.sol.health.backlog.thinking.days.incomplete = { $thinking_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }al menos { $thinking } segmento en espera de razonamiento a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $thinking } segmento en espera de razonamiento a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $thinking } segmento en espera de razonamiento a lo largo de { $days } días (estado incompleto)
    }
    [many] { $days_quantity ->
    [one] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento a lo largo de { $days } días (estado incompleto)
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento a lo largo de { $days } días (estado incompleto)
    }
    }

journal.cli.sol.health.backlog.thinking.media.days = { $thinking_quantity ->
    [one] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segmento en espera de razonamiento, { $media } segmento en espera de procesamiento de medios a lo largo de { $days } día
    [many] { $space_2 }{ $thinking } segmento en espera de razonamiento, { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días
   *[other] { $space_2 }{ $thinking } segmento en espera de razonamiento, { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días
    }
    [many] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segmento en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día
    [many] { $space_2 }{ $thinking } segmento en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
   *[other] { $space_2 }{ $thinking } segmento en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segmento en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día
    [many] { $space_2 }{ $thinking } segmento en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
   *[other] { $space_2 }{ $thinking } segmento en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
    }
    }
    [many] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmento en espera de procesamiento de medios a lo largo de { $days } día
    [many] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días
   *[other] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días
    }
    [many] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día
    [many] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
   *[other] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día
    [many] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
   *[other] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
    }
    }
   *[other] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmento en espera de procesamiento de medios a lo largo de { $days } día
    [many] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días
   *[other] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días
    }
    [many] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día
    [many] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
   *[other] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día
    [many] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
   *[other] { $space_2 }{ $thinking } segmentos en espera de razonamiento, { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días
    }
    }
    }

journal.cli.sol.health.backlog.thinking.media.days.incomplete = { $thinking_quantity ->
    [one] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }al menos { $thinking } segmento en espera de razonamiento, al menos { $media } segmento en espera de procesamiento de medios a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $thinking } segmento en espera de razonamiento, al menos { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $thinking } segmento en espera de razonamiento, al menos { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
    }
    [many] { $days_quantity ->
    [one] { $space_2 }al menos { $thinking } segmento en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $thinking } segmento en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $thinking } segmento en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }al menos { $thinking } segmento en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $thinking } segmento en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $thinking } segmento en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
    }
    }
    [many] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmento en espera de procesamiento de medios a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
    }
    [many] { $days_quantity ->
    [one] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
    }
    }
   *[other] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmento en espera de procesamiento de medios a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmento en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
    }
    [many] { $days_quantity ->
    [one] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } día (estado incompleto)
    [many] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
   *[other] { $space_2 }al menos { $thinking } segmentos en espera de razonamiento, al menos { $media } segmentos en espera de procesamiento de medios a lo largo de { $days } días (estado incompleto)
    }
    }
    }

journal.cli.sol.health.backlog.unavailable = { $space_2 }Estado del análisis de segmentos no disponible

journal.cli.sol.health.consumer = Señales de consumo

journal.cli.sol.health.coverage.ratio = { $space_2 }{ $coverage_ratio }: { $value }

journal.cli.sol.health.facets.silent.past.day = { $space_2 }{ $facets_silent_24h }: { $value }

journal.cli.sol.health.facets.with.recent.capture = { $space_2 }{ $facets_with_recent_capture }: { $value }

journal.cli.sol.health.hours.total = { $space_2 }{ $hours_total }: { $value }

journal.cli.sol.health.hours.with.capture = { $space_2 }{ $hours_with_capture }: { $value }

journal.cli.sol.health.index = { $space_2 }search_index: { $state } (desactualizado { $stale }, ausente { $missing }, huérfano { $orphaned }, fallido { $failed })

journal.cli.sol.health.index.activity.at = { $space_2 }{ $index_activity_at }: { $value }

journal.cli.sol.health.intake = Ingesta

journal.cli.sol.health.last.segment.at = { $space_2 }{ $last_segment_at }: { $value }

journal.cli.sol.health.none = { $space_2 }ninguno

journal.cli.sol.health.notes = Notas

journal.cli.sol.health.profile.entities.total = { $space_2 }{ $profile_entities_total }: { $value }

journal.cli.sol.health.range = Rango: { $start } -> { $end }

journal.cli.sol.health.silent = Detalle de facetas en silencio

journal.cli.sol.health.synthesis = Síntesis

journal.cli.sol.health.talent.degraded.outputs.past.day = { $space_2 }{ $talent_degraded_outputs_24h }: { $value }

journal.cli.sol.health.talent.run.failures.past.day = { $space_2 }{ $talent_run_failures_24h }: { $value }

journal.cli.sol.help.apps = Aplicaciones ({ $invocation }):

journal.cli.sol.help.authority = Autoridad: { $path }

journal.cli.sol.help.command.activities.create = Crear un registro de actividad sintética nuevo a partir de los flags de { $argv } o de { $json } en { $stdin }.

journal.cli.sol.help.command.activities.get = Obtener un registro de actividad por ID.

journal.cli.sol.help.command.activities.list = Listar los registros de actividad de un día o de un rango de días inclusivo.

journal.cli.sol.help.command.activities.mute = Ocultar un registro de actividad sin eliminarlo.

journal.cli.sol.help.command.activities.unmute = Restaurar un registro de actividad que estaba oculto.

journal.cli.sol.help.command.activities.update = Aplicar un flag de { $argv } o un parche { $json } de { $stdin } a un registro de actividad.

journal.cli.sol.help.command.awareness.imports = Leer o actualizar el estado del seguimiento de importaciones.

journal.cli.sol.help.command.awareness.log = Agregar una entrada al registro diario de conciencia.

journal.cli.sol.help.command.awareness.log.read = Leer entradas del registro diario de conciencia.

journal.cli.sol.help.command.awareness.status = Mostrar el estado actual de conciencia.

journal.cli.sol.help.command.body.day = Mostrar un día de datos de salud importados.

journal.cli.sol.help.command.body.status = Mostrar el estado de los datos de salud importados.

journal.cli.sol.help.command.body.window = Mostrar el contexto de salud importado para una ventana de tiempo.

journal.cli.sol.help.command.entities.accept.merge.candidate = Previsualizar o aceptar un candidato de fusión de entidades registrado.

journal.cli.sol.help.command.entities.aka = Agregar un alias a una entidad asociada.

journal.cli.sol.help.command.entities.ambiguities = Listar ambigüedades de resolución de entidades que se pueden revisar.

journal.cli.sol.help.command.entities.attach = Asociar una entidad de forma permanente a una faceta.

journal.cli.sol.help.command.entities.delete = Mover una entidad y sus enlaces de faceta a la papelera; { $erase_flag } la borra de inmediato. Con { $erase_flag }, { $from_backups_flag } también la quita de tus copias de seguridad y { $not_from_backups_flag } la deja ahí. Sin ninguna de las dos, lo decide el ajuste de la papelera.

journal.cli.sol.help.command.entities.detect = Registrar una entidad detectada para un día en una faceta.

journal.cli.sol.help.command.entities.dismiss.merge.candidate = Descartar un candidato de fusión de entidades registrado.

journal.cli.sol.help.command.entities.entity.history = Mostrar versiones de identidad duraderas (no el historial de relaciones).

journal.cli.sol.help.command.entities.history = Mostrar la evidencia de conexión registrada para un par de entidades.

journal.cli.sol.help.command.entities.list = Listar las entidades de una faceta.

journal.cli.sol.help.command.entities.merge = Planear o aplicar una fusión de entidades del diario.

journal.cli.sol.help.command.entities.merge.candidates = Listar candidatos de fusión de entidades registrados.

journal.cli.sol.help.command.entities.move = Mover una entidad de una faceta a otra.

journal.cli.sol.help.command.entities.network = Mostrar las conexiones registradas a un salto de una entidad.

journal.cli.sol.help.command.entities.observations = Listar observaciones de una entidad asociada.

journal.cli.sol.help.command.entities.observe = Agregar una observación a una entidad asociada.

journal.cli.sol.help.command.entities.overview = Mostrar un panorama global de las conexiones registradas.

journal.cli.sol.help.command.entities.record.merge.candidate = Registrar una fusión de entidades propuesta (variante de origen -> destino canónico).

journal.cli.sol.help.command.entities.resolve.ambiguity = Resolver o reemplazar una elección de ambigüedad guardada.

journal.cli.sol.help.command.entities.resolve.ambiguity.group = Resolver un grupo de tarjetas de ambigüedad para un candidato de entidad.

journal.cli.sol.help.command.entities.restore.review = Restaurar un elemento de revisión apartado o deshacer una elección automática.

journal.cli.sol.help.command.entities.restore.version = Restaurar una versión de identidad ordinaria.

journal.cli.sol.help.command.entities.review = Listar tarjetas de revisión de entidades agrupadas y elementos apartados.

journal.cli.sol.help.command.entities.search = Buscar entidades por texto, tipo, faceta o actividad.

journal.cli.sol.help.command.entities.sweep.review = Ejecutar el barrido de la política de revisión de entidades.

journal.cli.sol.help.command.entities.update = Actualizar la descripción de una entidad.

journal.cli.sol.help.command.facets.accept = Aceptar un candidato de revisión de faceta.

journal.cli.sol.help.command.facets.dismiss = Descartar un candidato de revisión de faceta.

journal.cli.sol.help.command.facets.list.candidates = Listar candidatos de revisión de faceta registrados.

journal.cli.sol.help.command.health.for.range = Generar el informe de confianza de los datos del diario para un rango de días inclusivo.

journal.cli.sol.help.command.health.full = Generar el informe completo de confianza de los datos del diario para un día.

journal.cli.sol.help.command.health.pipeline = Envoltura delgada alrededor de { $pipeline_module }. Para las revisiones de confianza de los datos del diario usa `{ $summary_verb }` / `{ $full_verb }` / `{ $for_range_verb }`.

journal.cli.sol.help.command.health.summary = Resumir las señales de confianza de los datos del diario para un día.

journal.cli.sol.help.command.import.top.level = Importar contenido multimedia a través del diario

journal.cli.sol.help.command.journal.agents = Listar las salidas de agente disponibles para un día.

journal.cli.sol.help.command.journal.facet.create = Crear una faceta nueva.{ $line_break }{ $line_break }{ $consent_flag } registra en el registro de auditoría de la faceta que el propietario lo aprobó. Un{ $line_break }agente lo pasa después de que el propietario aprueba un cambio que el agente propuso, no para{ $line_break }uno que el propietario pidió. No cambia lo que se crea.

journal.cli.sol.help.command.journal.facet.delete = Mover una faceta y todo lo que contiene a la papelera, de donde se puede recuperar hasta que se borre.{ $line_break }{ $line_break }{ $yes_flag } es obligatorio; sin él, el comando se niega. Cada eliminación queda registrada en{ $line_break }el registro de acciones del diario como aprobada por el propietario, y { $consent_flag } se acepta pero no cambia{ $line_break }nada.

journal.cli.sol.help.command.journal.facet.mute = Silenciar una faceta (ocultarla de los listados predeterminados).

journal.cli.sol.help.command.journal.facet.rename = Renombrar una faceta.{ $line_break }{ $line_break }{ $consent_flag } registra en el registro de auditoría de la faceta que el propietario lo aprobó. Un{ $line_break }agente lo pasa después de que el propietario aprueba un cambio que el agente propuso, no para{ $line_break }uno que el propietario pidió. No cambia el cambio de nombre.

journal.cli.sol.help.command.journal.facet.show = Mostrar el resumen de la faceta.

journal.cli.sol.help.command.journal.facet.unmute = Quitar el silencio de una faceta (mostrarla en los listados predeterminados).

journal.cli.sol.help.command.journal.facet.update = Actualizar la configuración de la faceta.

journal.cli.sol.help.command.journal.facets = Listar las facetas.

journal.cli.sol.help.command.journal.import = Mostrar los metadatos completos de una importación.

journal.cli.sol.help.command.journal.imports = Listar las importaciones recientes con sus metadatos.

journal.cli.sol.help.command.journal.read = Leer el contenido completo de una salida de agente.

journal.cli.sol.help.command.journal.retention.config = Mostrar o actualizar la configuración de retención.

journal.cli.sol.help.command.journal.retention.list = Listar el contenido multimedia original listo para quitar.

journal.cli.sol.help.command.journal.search = Buscar en el índice del diario.{ $line_break }{ $line_break }Usa de 2 a 4 términos de contenido en lugar de preguntas en lenguaje natural; las palabras de pregunta{ $line_break }como { $example_terms } agregan ruido en este índice de palabras clave/{ $bm25 }. Sintaxis: { $or_operator } para{ $line_break }cualquier término, frases entre comillas para el texto exacto y { $wildcard } para coincidencias de prefijo.{ $line_break }{ $n_flag }/{ $limit_flag } es el máximo de resultados por día. Los resultados se agrupan por día,{ $line_break }el más reciente primero, hasta 20 días por página. { $offset_flag } omite días. { $d_flag }/{ $day_flag } es un{ $line_break }día exacto y no se puede combinar con { $day_from_flag } ni { $day_to_flag }. Cuando nada{ $line_break }coincide con todos los términos dentro de los otros filtros (faceta, agente, flujo, tramo de{ $line_break }tiempo) en todo el diario, la respuesta se marca { $relaxed } y esos{ $line_break }aciertos son coincidencias más débiles; ese juicio ocurre antes del filtro de fecha, así que un{ $line_break }día vacío o un rango vacío no significa que el rango nunca mencione los términos. Amplía{ $line_break }quitando términos, luego { $or_operator } y luego { $wildcard }. { $json_flag } no cambia la salida{ $line_break }porque es { $json } de cualquier modo.{ $line_break }Lee un resultado tal como se indexó por última vez con `{ $command }`. Copia los tres campos del mismo resultado; no uses su id de presentación como ruta de archivo.

journal.cli.sol.help.command.journal.storage.summary = Mostrar el resumen de almacenamiento del diario.

journal.cli.sol.help.command.link.authorized.clients = Listar cada certificado de cliente autorizado: huella, etiqueta, última vez que se vio (vista plana).

journal.cli.sol.help.command.link.confirm = confirmar la marca del diario para un enlace que la espera

journal.cli.sol.help.command.link.join = unirse a un { $solstone } con un código corto o un enlace de vinculación

journal.cli.sol.help.command.link.list = Imprimir cada dispositivo vinculado con la última vez que se vio.

journal.cli.sol.help.command.link.local.network.close = Cerrar tu diario a los dispositivos de tu red.

journal.cli.sol.help.command.link.local.network.open = Abrir tu diario a los dispositivos de tu red.

journal.cli.sol.help.command.link.local.network.status = Mostrar si tu diario está abierto a los dispositivos de tu red.

journal.cli.sol.help.command.link.observer.pause = pausar dispositivos vinculados (todavía no disponible).

journal.cli.sol.help.command.link.pair = Iniciar un enlace de vinculación, imprimir credenciales listas para unirse y, de forma opcional, esperar.

journal.cli.sol.help.command.link.private.link.disable = Desactivar la red privada.

journal.cli.sol.help.command.link.private.link.setup = Configurar la red privada.

journal.cli.sol.help.command.link.private.link.status = Mostrar el estado de la red privada.

journal.cli.sol.help.command.link.serve = servir un diario vinculado por el puente de enlace local

journal.cli.sol.help.command.link.set.label = Definir o quitar la etiqueta del propietario para un dispositivo vinculado.

journal.cli.sol.help.command.link.status = Informar la inscripción, el estado de { $listen_ws }, la cantidad de túneles activos y el punto de conexión del relé.

journal.cli.sol.help.command.link.top.level.status = mostrar el estado del enlace y la versión observada del diario remoto

journal.cli.sol.help.command.link.unpair = Revocar un dispositivo vinculado. La próxima reconexión desde ese dispositivo falla en el saludo { $tls }.

journal.cli.sol.help.command.moved.navigate = Se movió a `{ $command }`.

journal.cli.sol.help.command.settings.convey.status = Mostrar el estado de la red de { $convey } y de la URL del panel.

journal.cli.sol.help.command.settings.identity.set = Actualizar la identidad del propietario del diario.

journal.cli.sol.help.command.settings.identity.show = Mostrar la configuración de identidad del diario.

journal.cli.sol.help.command.settings.keys.clear = Quitar una clave de API de la configuración del diario.

journal.cli.sol.help.command.settings.keys.set = Definir una clave de API en la configuración del diario.

journal.cli.sol.help.command.settings.keys.show = Mostrar el estado de las claves de API configuradas.

journal.cli.sol.help.command.settings.keys.validate = Validar todas las claves de API configuradas sin guardar de manera predeterminada.

journal.cli.sol.help.command.settings.processing.set = Definir el modo de procesamiento diferido y la ventana de vaciado.

journal.cli.sol.help.command.settings.processing.show = Mostrar la configuración efectiva del procesamiento diferido.

journal.cli.sol.help.command.settings.show = Mostrar un resumen de la configuración del diario.

journal.cli.sol.help.command.settings.transcribe.set.backend = Definir el backend de transcripción.

journal.cli.sol.help.command.settings.transcribe.show = Mostrar la configuración del backend de transcripción.

journal.cli.sol.help.command.speakers.attribute.segment = Ejecutar la atribución de hablante (capas 1-3) en un solo segmento.{ $line_break }{ $line_break }Clasifica cada oración con la detección del propietario, heurísticas estructurales{ $line_break }y la coincidencia acústica de huellas de voz. De forma opcional escribe { $labels_file }{ $line_break }y acumula huellas de voz de alta confianza.

journal.cli.sol.help.command.speakers.backfill = Ejecutar la atribución de hablante en todos los segmentos con vectores.{ $line_break }{ $line_break }Procesa los segmentos del más antiguo al más reciente para construir las huellas de voz de forma progresiva.{ $line_break }Omite los segmentos que ya tienen { $labels_file } (se puede volver a ejecutar).

journal.cli.sol.help.command.speakers.backfill.last.seen = Rellenar { $last_seen_field } en las filas de metadatos de huellas de voz existentes.

journal.cli.sol.help.command.speakers.backfill.resume = Reanudar una operación de relleno de atribución de hablante interrumpida o fallida.

journal.cli.sol.help.command.speakers.backfill.status = Mostrar el estado de una operación de relleno de atribución de hablante.

journal.cli.sol.help.command.speakers.bootstrap = Inicializar huellas de voz a partir de segmentos de un solo hablante.{ $line_break }{ $line_break }Recorre todo el diario en busca de segmentos donde { $speakers_file } lista exactamente{ $line_break }un hablante. En esos segmentos, todos los vectores que no son del propietario pertenecen a ese{ $line_break }hablante. Los guarda como huellas de voz usando el centroide del propietario para{ $line_break }restar al propietario.

journal.cli.sol.help.command.speakers.build.from.tags = Construir el centroide de voz del propietario a partir de etiquetas manuales del propietario validadas.

journal.cli.sol.help.command.speakers.confirm.owner = Confirmar en el navegador el candidato de voz del propietario.{ $line_break }{ $line_break }El { $confirm_verb } nativo no guarda un centroide. Revisa y confirma los candidatos en el navegador en { $speakers_page }.

journal.cli.sol.help.command.speakers.correct = Corregir de inmediato una atribución de hablante.

journal.cli.sol.help.command.speakers.day.segments = Listar los segmentos de revisión de hablante de un día.

journal.cli.sol.help.command.speakers.detect = Ejecutar la detección del candidato de voz del propietario.

journal.cli.sol.help.command.speakers.discover = Descubrir hablantes desconocidos que se repiten entre segmentos.

journal.cli.sol.help.command.speakers.dismiss.cluster = Descartar un clúster de descubrimiento actual de las superficies de listado.

journal.cli.sol.help.command.speakers.dismissals = Listar los descartes de clústeres de hablante que quedaron plegados.

journal.cli.sol.help.command.speakers.identify = Identificar un clúster de hablante desconocido que se descubrió.

journal.cli.sol.help.command.speakers.identify.operation = Mostrar una operación de identificación de hablante censurada.

journal.cli.sol.help.command.speakers.identify.operations = Listar operaciones de identificación de hablante censuradas.

journal.cli.sol.help.command.speakers.identify.undo = Deshacer una operación de identificación de hablante aplicada.

journal.cli.sol.help.command.speakers.keep.separate.list = Listar las afirmaciones de mantener separados a hablantes que quedaron plegadas.

journal.cli.sol.help.command.speakers.link.import = Asociar el nombre de un participante de una importación como alias de una entidad existente.

journal.cli.sol.help.command.speakers.merge.names = Fusionar una variante de nombre de hablante en una entidad canónica.

journal.cli.sol.help.command.speakers.owner.ready = Revisar si la detección de la voz del propietario debe mostrarse a la persona.

journal.cli.sol.help.command.speakers.presence = Mostrar quién estaba cerca de un clúster de voz desconocida que se descubrió.

journal.cli.sol.help.command.speakers.propagate.correction = Previsualizar o aplicar la propagación acotada de una corrección.{ $line_break }{ $line_break }Para revertir se usa el mismo verbo con los dos id de hablante intercambiados.

journal.cli.sol.help.command.speakers.rebuild.owner = Reconstruir el centroide de voz del propietario a partir de las etiquetas manuales actuales del propietario.

journal.cli.sol.help.command.speakers.reject.owner = Rechazar la generación presentada del candidato de voz del propietario.

journal.cli.sol.help.command.speakers.repair = Reparar la contaminación de hablantes que no son { $person_type } sin volver a aprender de ella.{ $line_break }{ $line_break }Revisa identidades y huellas de voz, quita la contaminación de huellas de voz que no son { $person_type }{ $line_break }y actualiza las etiquetas de segmento con una atribución nueva según el estado actual.

journal.cli.sol.help.command.speakers.repair.resume = Reanudar una operación de reparación de hablante interrumpida o fallida.

journal.cli.sol.help.command.speakers.repair.status = Mostrar el estado de una operación de reparación de hablante.

journal.cli.sol.help.command.speakers.resolve.names = Resolver variantes de nombre de hablante usando la similitud de huellas de voz.{ $line_break }{ $line_break }Compara los centroides de huellas de voz entre todas las entidades. Los pares con similitud de coseno{ $line_break }mayor que 0.90 se marcan como la misma persona. Las variantes sin ambigüedad{ $line_break }(el nombre corto es la primera palabra del nombre completo) se fusionan solas al agregar el{ $line_break }nombre corto como alias de la entidad canónica.

journal.cli.sol.help.command.speakers.seed.from.imports = Sembrar huellas de voz a partir de segmentos importados con transcripciones atribuidas a un hablante.{ $line_break }{ $line_break }Recorre los flujos de importación en busca de segmentos que tengan { $transcript_file }{ $line_break }(con etiquetas de hablante) y vectores de audio. Asigna cada vector a un hablante{ $line_break }por alineación en el tiempo, empareja los hablantes con entidades existentes y guarda{ $line_break }los vectores como huellas de voz con protección contra la contaminación del propietario.

journal.cli.sol.help.command.speakers.sentences = Listar los id de oración de una fuente de revisión de hablante.

journal.cli.sol.help.command.speakers.status = Mostrar el estado del subsistema de hablantes como { $json }.

journal.cli.sol.help.command.speakers.suggest = Sugerir oportunidades de curaduría de hablantes.

journal.cli.sol.help.command.speakers.tag.owner = Etiquetar una oración como la voz del propietario.

journal.cli.sol.help.command.speakers.wipe = Mover todos los archivos de voz de hablante del diario a la papelera.{ $line_break }{ $line_break }Sin { $commit_flag }, imprime un informe de lo que se movería. Con{ $line_break }{ $commit_flag }, mueve los NPZ de vectores de segmento, las etiquetas y correcciones de hablante,{ $line_break }las huellas de voz de cada entidad, los centroides del propietario, la instantánea del candidato del propietario{ $line_break }y el conjunto de candidatos a la papelera, de donde se pueden recuperar hasta{ $line_break }que se borren. { $erase_flag } los borra de inmediato en su lugar.{ $line_break }{ $line_break }Con { $erase_flag }, { $from_backups_flag } también los quita de tus copias de seguridad y{ $line_break }{ $not_from_backups_flag } los deja ahí. Sin ninguna de las dos, lo decide el ajuste de{ $line_break }la papelera.

journal.cli.sol.help.command.status.top.level = Mostrar el estado de red del diario.

journal.cli.sol.help.command.thinking.clear.local.endpoint = Quitar el punto de conexión del proveedor local BYO.

journal.cli.sol.help.command.thinking.confidential.disable = Desactivar el procesamiento confidencial.

journal.cli.sol.help.command.thinking.confidential.enable = Activar el procesamiento confidencial.

journal.cli.sol.help.command.thinking.confidential.recheck = Volver a revisar el procesamiento confidencial.

journal.cli.sol.help.command.thinking.confidential.status = Mostrar el estado del procesamiento confidencial.

journal.cli.sol.help.command.thinking.keys.clear = Quitar una clave de IA de la configuración del diario.

journal.cli.sol.help.command.thinking.keys.set = Definir una clave de IA en la configuración del diario.

journal.cli.sol.help.command.thinking.keys.show = Mostrar el estado de las claves de IA configuradas.

journal.cli.sol.help.command.thinking.keys.validate = Validar las claves de IA configuradas.

journal.cli.sol.help.command.thinking.local.availability = Mostrar la disponibilidad del modelo local.

journal.cli.sol.help.command.thinking.local.bootstrap = Iniciar la configuración del modelo local.

journal.cli.sol.help.command.thinking.local.bootstrap.cancel = Cancelar la configuración del modelo local.

journal.cli.sol.help.command.thinking.local.bootstrap.status = Mostrar el estado de la configuración local.

journal.cli.sol.help.command.thinking.local.models = Listar los modelos locales.

journal.cli.sol.help.command.thinking.local.readiness = Mostrar si el proveedor local está listo.

journal.cli.sol.help.command.thinking.local.status = Mostrar el estado del proveedor local.

journal.cli.sol.help.command.thinking.providers.set.active = Definir el proveedor y el modelo que se usan para todo el razonamiento.

journal.cli.sol.help.command.thinking.providers.show = Mostrar la configuración del proveedor.

journal.cli.sol.help.command.thinking.set.local.endpoint = Definir el punto de conexión del proveedor local BYO.

journal.cli.sol.help.command.transcripts.read = Leer el contenido de la transcripción de un día, un segmento o un rango de tiempo.

journal.cli.sol.help.command.transcripts.scan = Listar los rangos de cobertura de la transcripción de un día.

journal.cli.sol.help.command.transcripts.segments = Listar los segmentos de grabación de un día.

journal.cli.sol.help.command.transcripts.speakers = Listar las filas de corrección de hablante de un segmento de transcripción.

journal.cli.sol.help.command.transcripts.stats = Mostrar los conteos diarios de cobertura de la transcripción de un mes.

journal.cli.sol.help.command.usage = Uso: { $syntax }

journal.cli.sol.help.computer = Esta computadora

journal.cli.sol.help.journal = Tu diario

journal.cli.sol.help.operation = Operación: { $operation }

journal.cli.sol.help.parameter = { $shape ->
   *[count] { $syntax } [cantidad]
    [multiple] { $syntax } [múltiple]
    [multiple_count] { $syntax } [múltiple, cantidad]
    [required] { $syntax } [obligatorio]
    [required_count] { $syntax } [obligatorio, cantidad]
    [required_multiple] { $syntax } [obligatorio, múltiple]
    [required_multiple_count] { $syntax } [obligatorio, múltiple, cantidad]
    }

journal.cli.sol.help.parameter.default = { $shape ->
   *[plain] { $syntax } [predeterminado: { $default_value }]
    [count] { $syntax } [predeterminado: { $default_value }, cantidad]
    [multiple] { $syntax } [predeterminado: { $default_value }, múltiple]
    [multiple_count] { $syntax } [predeterminado: { $default_value }, múltiple, cantidad]
    [required] { $syntax } [obligatorio, predeterminado: { $default_value }]
    [required_count] { $syntax } [obligatorio, predeterminado: { $default_value }, cantidad]
    [required_multiple] { $syntax } [obligatorio, predeterminado: { $default_value }, múltiple]
    [required_multiple_count] { $syntax } [obligatorio, predeterminado: { $default_value }, múltiple, cantidad]
    }

journal.cli.sol.help.parameter.default.flag = { $shape ->
   *[plain] { $syntax } [predeterminado: { $default_value }, valor del flag: { $flag_value }]
    [count] { $syntax } [predeterminado: { $default_value }, valor del flag: { $flag_value }, cantidad]
    [multiple] { $syntax } [predeterminado: { $default_value }, valor del flag: { $flag_value }, múltiple]
    [multiple_count] { $syntax } [predeterminado: { $default_value }, valor del flag: { $flag_value }, múltiple, cantidad]
    [required] { $syntax } [obligatorio, predeterminado: { $default_value }, valor del flag: { $flag_value }]
    [required_count] { $syntax } [obligatorio, predeterminado: { $default_value }, valor del flag: { $flag_value }, cantidad]
    [required_multiple] { $syntax } [obligatorio, predeterminado: { $default_value }, valor del flag: { $flag_value }, múltiple]
    [required_multiple_count] { $syntax } [obligatorio, predeterminado: { $default_value }, valor del flag: { $flag_value }, múltiple, cantidad]
    }

journal.cli.sol.help.parameter.flag = { $shape ->
   *[plain] { $syntax } [valor del flag: { $flag_value }]
    [count] { $syntax } [valor del flag: { $flag_value }, cantidad]
    [multiple] { $syntax } [valor del flag: { $flag_value }, múltiple]
    [multiple_count] { $syntax } [valor del flag: { $flag_value }, múltiple, cantidad]
    [required] { $syntax } [obligatorio, valor del flag: { $flag_value }]
    [required_count] { $syntax } [obligatorio, valor del flag: { $flag_value }, cantidad]
    [required_multiple] { $syntax } [obligatorio, valor del flag: { $flag_value }, múltiple]
    [required_multiple_count] { $syntax } [obligatorio, valor del flag: { $flag_value }, múltiple, cantidad]
    }

journal.cli.sol.help.section.arguments = Argumentos:

journal.cli.sol.help.section.commands = Comandos:

journal.cli.sol.help.section.options = Opciones:

journal.cli.sol.help.title = { $solstone_name } - tu diario, desde la línea de comandos

journal.cli.sol.help.tools = Herramientas

journal.cli.sol.import.already.started = ya iniciado{ $line_break }

journal.cli.sol.import.argument.media = los siguientes argumentos son obligatorios: media

journal.cli.sol.import.argument.unexpected = argumento(s) inesperado(s): { $arguments }

journal.cli.sol.import.argument.unknown = argumentos no reconocidos: { $token }

journal.cli.sol.import.duplicate.entries = { $entries_quantity ->
    [one] { $solstone_import }: ya se importó el { $date } ({ $entries } entrada); se omite
    [many] { $solstone_import }: ya se importó el { $date } ({ $entries } entradas); se omite
   *[other] { $solstone_import }: ya se importó el { $date } ({ $entries } entradas); se omite
    }

journal.cli.sol.import.duplicate.entries.literal = { $solstone_import }: ya se importó el { $date } ({ $entries } entradas); se omite{ $line_break }

journal.cli.sol.import.duplicate.entries.literal.unknown = { $solstone_import }: ya se importó en fecha desconocida ({ $entries } entradas); se omite{ $line_break }

journal.cli.sol.import.duplicate.entries.unknown = { $entries_quantity ->
    [one] { $solstone_import }: ya se importó en fecha desconocida ({ $entries } entrada); se omite
    [many] { $solstone_import }: ya se importó en fecha desconocida ({ $entries } entradas); se omite
   *[other] { $solstone_import }: ya se importó en fecha desconocida ({ $entries } entradas); se omite
    }

journal.cli.sol.import.duplicate.imported = { $solstone_import }: ya se importó el { $date }; se omite{ $line_break }

journal.cli.sol.import.duplicate.imported.unknown = { $solstone_import }: ya se importó en fecha desconocida; se omite{ $line_break }

journal.cli.sol.import.duplicate.skipping = { $solstone_import }: importación duplicada; se omite{ $line_break }

journal.cli.sol.import.duplicate.staged = { $solstone_import }: ya se preparó como { $import_id }; se omite{ $line_break }

journal.cli.sol.import.duplicate.staged.unknown = { $solstone_import }: ya se preparó como desconocido; se omite{ $line_break }

journal.cli.sol.import.help = uso: { $solstone_import_h_timestamp_ti }{ $line_break }{ $line_break }Importar contenido multimedia a través del diario{ $line_break }

journal.cli.sol.import.host.guidance = `{ $auto } { $guidance }` requiere el host del diario. Usa `{ $timestamp }` aquí o ejecuta `{ $solstone_journal_importer }`.

journal.cli.sol.import.host.option = `{ $option }` requiere el host del diario. Ejecuta esto en el host del diario con `{ $solstone_journal_importer }`.

journal.cli.sol.import.journal.unreachable = { $solstone_import }: no se pudo conectar con el diario. Inícialo con '{ $solstone_journal_up }' y vuelve a intentarlo.{ $line_break }

journal.cli.sol.import.notice.failed = no se pudo mostrar el aviso del día de importación: { $error }

journal.cli.sol.import.queued = procesamiento en cola{ $line_break }

journal.cli.sol.import.response.malformed = { $solstone_import }: no se pudo leer la respuesta del diario{ $line_break }

journal.cli.sol.import.size.failed = no se pudo mostrar el tamaño de la importación: { $error }

journal.cli.sol.import.size.skipped = { $skipped } omitidos{ $line_break }

journal.cli.sol.import.size.total = { $recordings_quantity ->
    [one] { $recordings } grabación en { $days_quantity ->
    [one] { $days } día
    [many] { $days } días
   *[other] { $days } días
    }
    [many] { $recordings } grabaciones en { $days_quantity ->
    [one] { $days } día
    [many] { $days } días
   *[other] { $days } días
    }
   *[other] { $recordings } grabaciones en { $days_quantity ->
    [one] { $days } día
    [many] { $days } días
   *[other] { $days } días
    }
    }

journal.cli.sol.import.stage.failed = { $solstone_import }: no se pudo preparar la importación: { $error }{ $line_break }

journal.cli.sol.import.staged.failed = { $solstone_import }: se preparó { $staged_path } pero el procesamiento no quedó en cola: { $error }{ $line_break }

journal.cli.sol.import.staged.malformed = { $solstone_import }: se preparó { $staged_path } pero el procesamiento no quedó en cola: no se pudo leer la respuesta del diario{ $line_break }

journal.cli.sol.import.staged.path = preparado { $path }{ $line_break }

journal.cli.sol.import.staged.timestamp = marca de tiempo { $timestamp }{ $line_break }

journal.cli.sol.import.staged.unreachable = { $solstone_import }: se preparó { $staged_path } pero el procesamiento no quedó en cola: no se pudo conectar con el diario{ $line_break }

journal.cli.sol.import.task.queued = tarea de procesamiento en cola { $task_id }{ $line_break }

journal.cli.sol.import.task.started = ya iniciado como tarea { $task_id }{ $line_break }

journal.cli.sol.import.usage.error = uso: { $solstone_import_h_timestamp_ti }{ $line_break }{ $line_break }Importar contenido multimedia a través del diario{ $line_break }{ $solstone_import }: error: { $error }{ $line_break }

journal.cli.sol.journal.facet.delete.yes = Error: { $yes } es obligatorio para eliminar una faceta.

journal.cli.sol.launcher.cycle.mac = { $solstone_name }: se detectó un ciclo de enlaces simbólicos del lanzador nativo. Vuelve a instalar la app del diario.

journal.cli.sol.launcher.cycle.unix = { $solstone_name }: se detectó un ciclo de enlaces simbólicos del lanzador nativo. Vuelve a instalar { $solstone_name } y { $sibling }.

journal.cli.sol.launcher.dangling.mac = { $solstone_name }: el enlace simbólico del lanzador nativo está colgante: { $path }. Vuelve a instalar la app del diario.

journal.cli.sol.launcher.dangling.unix = { $solstone_name }: el enlace simbólico del lanzador nativo está colgante: { $path }. Vuelve a instalar { $solstone_name } y { $sibling }.

journal.cli.sol.launcher.executable.mac = { $solstone_name }: el hermano nativo { $sibling } no es ejecutable: { $path }. Vuelve a instalar la app del diario.

journal.cli.sol.launcher.executable.unix = { $solstone_name }: el hermano nativo { $sibling } no es ejecutable: { $path }. Vuelve a instalar { $solstone_name } y { $sibling }.

journal.cli.sol.launcher.missing.mac = { $solstone_name }: falta el hermano nativo { $sibling }: { $path }. Vuelve a instalar la app del diario.

journal.cli.sol.launcher.missing.unix = { $solstone_name }: falta el hermano nativo { $sibling }: { $path }. Vuelve a instalar { $solstone_name } y { $sibling }.

journal.cli.sol.link.argument.expected = argumento { $option }: se esperaba un argumento

journal.cli.sol.link.argument.unrecognized = argumentos no reconocidos: { $unknown }

journal.cli.sol.link.bundle.chain.empty = { $chain_pem } no contiene certificados

journal.cli.sol.link.bundle.chain.incomplete = { $chain_pem } contiene un certificado incompleto

journal.cli.sol.link.bundle.endpoint.ip = al punto de conexión { $lan } le falta la ip

journal.cli.sol.link.bundle.endpoint.port = el punto de conexión { $lan } tiene un puerto no válido

journal.cli.sol.link.bundle.endpoints.list = { $peer_json } { $local_endpoints } debe ser una lista

journal.cli.sol.link.bundle.endpoints.objects = las entradas de { $peer_json } { $local_endpoints } deben ser objetos

journal.cli.sol.link.bundle.exists = La ruta de credenciales ya existe: { $path }. Quítala y vuelve a ejecutar si vas a vincular de nuevo.

journal.cli.sol.link.bundle.exists.spent = La ruta de credenciales ya existe: { $path }. El código de vinculación ya se usó; genera uno nuevo y vuelve a ejecutar después de quitarla.

journal.cli.sol.link.bundle.files.incomplete = el conjunto de archivos del paquete de credenciales está incompleto

journal.cli.sol.link.bundle.files.missing = falta el archivo de paquete { $pl }: { $paths }

journal.cli.sol.link.bundle.home.missing = no se pudo resolver el directorio de inicio para las credenciales de vinculación de este dispositivo.

journal.cli.sol.link.bundle.invalid = paquete de vinculación no válido para la etiqueta '{ $label }' en { $path }: { $error }. Ejecuta `{ $solstone_link_join }` para vincular este dispositivo.

journal.cli.sol.link.bundle.multiple = se encontraron varios paquetes de vinculación: { $labels }. pasa { $label } para elegir uno.

journal.cli.sol.link.bundle.none = no se encontraron paquetes de vinculación en { $path }. ejecuta `{ $solstone_link_join }` para vincular este dispositivo.

journal.cli.sol.link.bundle.path.missing = no se encontró el paquete { $pl }: { $path }

journal.cli.sol.link.bundle.peer.invalid = { $peer_json } no es válido en { $path }: { $error }

journal.cli.sol.link.confirm.done = la marca de tu diario ya está confirmada. no hay nada que hacer.{ $line_break }

journal.cli.sol.link.confirm.error = { $solstone_link_confirm }: error: { $error }{ $line_break }

journal.cli.sol.link.confirm.help = uso: { $solstone_link_confirm_h_label_ }{ $line_break }{ $line_break }Confirmar la marca del diario para un enlace que la espera.{ $line_break }{ $line_break }opciones:{ $line_break }{ $h_help }{ $space_5 }mostrar este mensaje de ayuda y salir{ $line_break }{ $label_label }{ $space_2 }Etiqueta del paquete de vinculación (predeterminado: el único enlace vinculado){ $line_break }{ $mark_mark }{ $space_4 }las dos palabras de la marca que muestra la app de red de tu diario.{ $line_break }{ $space_17 }hace falta cuando no hay una terminal en la que preguntarte.{ $line_break }

journal.cli.sol.link.confirm.success = vinculado.{ $line_break }

journal.cli.sol.link.confirm.usage.error = uso: { $solstone_link_confirm_h_label_ }{ $line_break }{ $solstone_link_confirm }: error: { $error }{ $line_break }

journal.cli.sol.link.held = esperando a que confirmes la marca de tu diario. nada de lo que espera entra en tu diario hasta que lo hagas.{ $line_break }cuando estés listo, ejecuta: { $solstone_link_confirm } { $label_1 } { $label }{ $line_break }

journal.cli.sol.link.join.code.invalid = El código de vinculación no coincide con una forma aceptada. Usa un enlace de vinculación como { $https_go_solstone_app_p }... de '{ $solstone_call_link_pair }'.

journal.cli.sol.link.join.code.malformed = Enlace de vinculación mal formado. Usa el valor completo { $https_go_solstone_app_p }... de la salida de la vinculación.

journal.cli.sol.link.join.code.nonlocal = El enlace de vinculación apunta a una dirección fuera de la red local a la que se conectará quien se une.

journal.cli.sol.link.join.help = uso: { $solstone_link_join_h_home_home }{ $line_break }{ $label_label_mark_mark }{ $line_break }{ $line_break }opciones:{ $line_break }{ $h_help }{ $space_5 }mostrar este mensaje de ayuda y salir{ $line_break }{ $home_home }{ $space_4 }Base del receptor { $url }{ $line_break }{ $code_code }{ $space_4 }enlace de vinculación { $url }{ $line_break }{ $as_as_role }{ $space_3 }Etiqueta opcional para unirse como{ $line_break }{ $label_label }{ $space_2 }Etiqueta local de credenciales (predeterminado: el nombre de este equipo){ $line_break }{ $mark_mark }{ $space_4 }las dos palabras de la marca que muestra la app de red de tu diario.{ $line_break }{ $space_17 }hace falta cuando no hay una terminal en la que preguntarte.{ $line_break }

journal.cli.sol.link.join.invalid.role = rol no válido; se esperaba uno de: { $phone }, { $observer }{ $line_break }

journal.cli.sol.link.join.lock.error = { $solstone_link_join }: error: { $err }{ $line_break }

journal.cli.sol.link.join.paired = vinculado.{ $line_break }credenciales: { $path }{ $line_break }

journal.cli.sol.link.join.pairing.unavailable = El mecanismo de vinculación no está disponible.{ $line_break }

journal.cli.sol.link.join.required.code = los siguientes argumentos son obligatorios: { $code }

journal.cli.sol.link.join.response.large = La respuesta de vinculación { $local_endpoints } es demasiado grande.{ $line_break }

journal.cli.sol.link.join.target.host = Al enlace de vinculación le falta el host.

journal.cli.sol.link.join.target.port = Al enlace de vinculación le falta el puerto explícito.

journal.cli.sol.link.join.usage.error = uso: { $solstone_link_join_h_home_home }{ $line_break }{ $label_label_mark_mark }{ $line_break }{ $solstone_link_join }: error: { $error }{ $line_break }

journal.cli.sol.link.label.character = { $label } solo puede contener letras, números, '-', '_' y '.'

journal.cli.sol.link.label.empty = { $label } no debe estar vacío

journal.cli.sol.link.label.length = { $label } debe tener 80 caracteres o menos

journal.cli.sol.link.label.separator = { $label } no debe contener separadores de ruta

journal.cli.sol.link.label.start = { $label } no debe empezar con '.'

journal.cli.sol.link.label.traversal = { $label } no debe contener '..'

journal.cli.sol.link.mark.ask.identified = ¿coincide con tu diario? escribe { $yes } o { $no }:

journal.cli.sol.link.mark.ask.unavailable = escribe { $continue } para vincular de todos modos, o { $cancel } para detener:

journal.cli.sol.link.mark.confirm.cancel = se canceló la vinculación. consigue un enlace de vinculación nuevo de tu diario y ejecuta { $solstone_link_join } de nuevo cuando estés listo.{ $line_break }

journal.cli.sol.link.mark.confirm.mismatch = no vinculado.{ $line_break }dijiste que esta marca no coincide con la que muestra tu diario, así que esta computadora no está vinculada. puede que hayas pegado el enlace equivocado, o algo no está bien. consigue un enlace de vinculación nuevo de tu diario y ejecuta { $solstone_link_join } de nuevo, o escribe a { $support_solstone_app } y te ayudamos.{ $line_break }

journal.cli.sol.link.mark.confirm.terminal = { $solstone_link_confirm } no puede preguntarte aquí por la marca de tu diario. ejecútalo en una terminal, o da las dos palabras de la marca con { $mark_word_word }.{ $line_break }

journal.cli.sol.link.mark.confirm.unavailable = esta computadora no pudo determinar la marca del diario, así que las palabras que diste no se pueden comparar. esperando a que confirmes la marca de tu diario. nada de lo que espera entra en tu diario hasta que lo hagas. ejecuta { $solstone_link_confirm } en una terminal para decidirlo tú.{ $line_break }

journal.cli.sol.link.mark.confirm.wrong = no vinculado.{ $line_break }las palabras de la marca que diste no coinciden con la marca del diario, así que esta computadora no está vinculada. revisa las palabras, consigue un enlace de vinculación nuevo de tu diario y ejecuta { $solstone_link_join } de nuevo.{ $line_break }

journal.cli.sol.link.mark.join.cancel = se canceló la vinculación. nada de lo que esta computadora ha guardado fue a ese diario por este enlace. consigue un enlace de vinculación nuevo de tu diario y ejecuta { $solstone_link_join } de nuevo cuando estés listo.{ $line_break }

journal.cli.sol.link.mark.join.mismatch = no vinculado.{ $line_break }dijiste que esta marca no coincide con la que muestra tu diario, así que esta computadora no está vinculada, y nada de lo que ha guardado fue a ese diario por este enlace. puede que hayas pegado el enlace equivocado, o algo no está bien. consigue un enlace de vinculación nuevo de tu diario y ejecuta { $solstone_link_join } de nuevo, o escribe a { $support_solstone_app } y te ayudamos.{ $line_break }

journal.cli.sol.link.mark.join.unverifiable = no se pudo verificar.{ $line_break }esta computadora no pudo determinar la marca del diario, así que las palabras que diste no se pueden comparar. esta computadora no está vinculada, y nada de lo que ha guardado fue a ese diario por este enlace. consigue un enlace de vinculación nuevo de tu diario y ejecuta { $solstone_link_join } en una terminal para decidirlo tú.{ $line_break }

journal.cli.sol.link.mark.join.wrong = no vinculado.{ $line_break }las palabras de la marca que diste no coinciden con la marca del diario, así que esta computadora no está vinculada, y nada de lo que ha guardado fue a ese diario por este enlace. revisa las palabras, consigue un enlace de vinculación nuevo de tu diario y ejecuta { $solstone_link_join } de nuevo.{ $line_break }

journal.cli.sol.link.mark.prompt.identified = un paso más: revisa la marca de tu diario.{ $line_break }{ $line_break }{ $space_2 }la marca de tu diario: { $spoken }{ $line_break }{ $line_break }tu diario muestra esta misma marca en su app de red. debe coincidir, exactamente.{ $line_break }¿coincide con tu diario? escribe { $yes } o { $no }:{ $line_break }

journal.cli.sol.link.mark.prompt.unavailable = un paso más: revisa la marca de tu diario.{ $line_break }{ $line_break }{ $space_2 }la marca de tu diario: no disponible en este momento{ $line_break }{ $line_break }no se pudo verificar.{ $line_break }esta computadora no pudo determinar la marca de tu diario, así que no hay nada que comparar. { $continue } solo si estás seguro de que el enlace vino de tu diario.{ $line_break }escribe { $continue } para vincular de todos modos, o { $cancel } para detener:{ $line_break }

journal.cli.sol.link.mark.terminal.required = { $solstone_link_join } no puede preguntarte aquí por la marca de tu diario. ejecútalo en una terminal, o da las dos palabras de la marca con { $mark_word_word }.{ $line_break }

journal.cli.sol.link.mark.usage = indica las dos palabras de la marca de tu diario, como { $mark_bramble_quokka }.{ $line_break }

journal.cli.sol.link.pair.closed = La vinculación falló: la ventana de vinculación está cerrada o vencida. Genera un código de vinculación nuevo y vuelve a intentarlo.

journal.cli.sol.link.pair.enrollment = El relé rechazó la inscripción del dispositivo ({ $endpoint } { $http } { $status }).

journal.cli.sol.link.pair.failed = La vinculación falló ({ $code }). Genera un código de vinculación nuevo y vuelve a intentarlo.

journal.cli.sol.link.pair.listener = No se pudo conectar con el receptor de vinculación.

journal.cli.sol.link.pair.rejected = La vinculación falló ({ $http } { $status }): la ventana de vinculación está cerrada o el código ya se usó.

journal.cli.sol.link.relay.abnormal = La conexión del relé se cerró de forma anómala. Vuelve a intentarlo o usa { $direct } en { $lan_vpn }.

journal.cli.sol.link.relay.closed = La ventana de vinculación del relé está cerrada. Vuelve a ejecutar { $solstone_link_join } con un código nuevo.

journal.cli.sol.link.relay.offline = el relé informa que tu diario está sin conexión. inicia el diario o usa { $direct } en lan/vpn.

journal.cli.sol.link.relay.overflow = El relé está sobrecargado de momento. Vuelve a intentarlo o usa { $direct } en { $lan_vpn }.

journal.cli.sol.link.relay.stalled = La conexión del relé se detuvo. Vuelve a intentarlo o usa { $direct } en { $lan_vpn }.

journal.cli.sol.link.relay.unauthorized = el relé rechazó el token de este dispositivo. vuelve a ejecutar { $solstone_link_join } para este dispositivo.

journal.cli.sol.link.relay.unknown = el relé no conoce este diario. vuelve a ejecutar { $solstone_link_join }.

journal.cli.sol.link.relay.unpaid = La cuenta del relé no está disponible. Revisa el estado del servicio o de la cuenta del relé, o usa { $direct }.

journal.cli.sol.link.relay.upgrade = El relé rechazó la actualización a WebSocket. Revisa { $relay_url } y vuelve a intentarlo.

journal.cli.sol.link.response.attestation = A la respuesta de vinculación le falta { $home_attestation }

journal.cli.sol.link.response.cert = A la respuesta de vinculación le falta { $client_cert }

journal.cli.sol.link.response.chain = A la respuesta de vinculación le falta { $ca_chain }

journal.cli.sol.link.response.endpoints = La respuesta de vinculación { $local_endpoints } debe ser un arreglo

journal.cli.sol.link.response.instance = A la respuesta de vinculación le falta { $instance_id }

journal.cli.sol.link.response.key = A la respuesta de vinculación le falta la clave de cliente generada

journal.cli.sol.link.serve.bind.failed = no se puede reservar { $value_127_0_0_1 }{ $port }: falló la reserva

journal.cli.sol.link.serve.bind.in.use = no se puede reservar { $value_127_0_0_1 }{ $port }: la dirección ya está en uso. Puede que otro `{ $solstone_link_serve }` o Convey ya esté usando ese puerto.

journal.cli.sol.link.serve.bridge.failed = Falló la configuración del puente nativo del enlace antes de servir. Vuelve a intentarlo después de volver a instalar { $solstone_core }.

journal.cli.sol.link.serve.credentials.invalid = Las credenciales de vinculación no son válidas. Ejecuta { $solstone_link_join } antes de { $solstone_link_serve }.

journal.cli.sol.link.serve.forward.direct = reenviando { $value_127_0_0_1 }{ $port } -> { $mark } por conexión directa{ $line_break }

journal.cli.sol.link.serve.forward.direct.unnamed = reenviando { $value_127_0_0_1 }{ $port } -> tu diario por conexión directa{ $line_break }

journal.cli.sol.link.serve.forward.either = reenviando { $value_127_0_0_1 }{ $port } -> { $mark } por conexión directa o por el relé{ $line_break }

journal.cli.sol.link.serve.forward.either.unnamed = reenviando { $value_127_0_0_1 }{ $port } -> tu diario por conexión directa o por el relé{ $line_break }

journal.cli.sol.link.serve.forward.relay = reenviando { $value_127_0_0_1 }{ $port } -> { $mark } solo por el relé{ $line_break }

journal.cli.sol.link.serve.forward.relay.unnamed = reenviando { $value_127_0_0_1 }{ $port } -> tu diario solo por el relé{ $line_break }

journal.cli.sol.link.serve.help = uso: { $solstone_link_serve_h_label_la }{ $line_break }{ $relay_url_relay_url_direct_rel }{ $line_break }{ $line_break }opciones:{ $line_break }{ $h_help }{ $space_12 }mostrar este mensaje de ayuda y salir{ $line_break }{ $label_label }{ $space_9 }Etiqueta del paquete de vinculación{ $line_break }{ $port_port }{ $space_11 }Puerto de loopback en el que servir (predeterminado: 5015){ $line_break }{ $relay_url_relay_url }{ $line_break }{ $space_24 }Reemplazar el { $url } del relé{ $line_break }{ $direct }{ $space_14 }Solo directo: llegar al diario por una conexión{ $line_break }{ $space_24 }directa, nunca por el relé. úsalo cuando tu diario esté{ $line_break }{ $space_24 }accesible de forma directa (la misma red o vpn) para evitar{ $line_break }{ $space_24 }depender del relé.{ $line_break }{ $relay_only }{ $space_10 }Solo relé: llegar al diario por el relé,{ $line_break }{ $space_24 }nunca por una conexión directa. úsalo cuando no se deban{ $line_break }{ $space_24 }intentar conexiones directas, aunque tu diario esté accesible{ $line_break }{ $space_24 }en la red local.{ $line_break }

journal.cli.sol.link.serve.mark.unnamed = este paquete de vinculación no tiene id de diario, así que no se puede nombrar. vuelve a ejecutar { $solstone_link_join }.{ $line_break }

journal.cli.sol.link.serve.policy.conflict = argumento { $direct }: no se permite con el argumento { $relay_only }

journal.cli.sol.link.serve.port.invalid = argumento { $port }: valor entero no válido: '{ $value }'

journal.cli.sol.link.serve.port.range = { $port } debe estar entre 1 y 65535

journal.cli.sol.link.serve.runtime.unavailable = El entorno de ejecución nativo del enlace no está disponible. Vuelve a instalar { $solstone_core } y vuelve a intentarlo.

journal.cli.sol.link.serve.unavailable = El mecanismo de servicio del enlace no está disponible.{ $line_break }

journal.cli.sol.link.serve.usage.error = uso: { $solstone_link_serve_h_label_la }{ $line_break }{ $relay_url_relay_url_direct_rel }{ $line_break }{ $solstone_link_serve }: error: { $error }{ $line_break }

journal.cli.sol.link.serve.warning = advertencia: solo para uso de desarrollo en sistemas de confianza.{ $line_break }este puente da a quienes llaman en local la autoridad completa del { $api } del diario del propietario vinculado.{ $line_break }ejecútalo solo donde confíes en todo el software que corre en este sistema.

journal.cli.sol.link.sighting.certainty = { $line_break }{ $line_break }esto es menos cierto que una conexión directa.

journal.cli.sol.link.sighting.claim.unshown = qué respondió:{ $space_2 }no se pudo mostrar la identidad declarada (declarada, no verificada)

journal.cli.sol.link.sighting.claimed = qué respondió:{ $space_2 }{ $spoken } (declarada, no verificada)

journal.cli.sol.link.sighting.direct = diario desconocido visto en { $addr }:

journal.cli.sol.link.sighting.journal = tu diario:{ $space_3 }{ $spoken }

journal.cli.sol.link.sighting.no.identity = qué respondió:{ $space_2 }no se presentó identidad

journal.cli.sol.link.sighting.relay = diario desconocido visto a través del relé:

journal.cli.sol.link.sighting.reset = { $line_break }{ $line_break }si hace poco restableciste tu diario, vuelve a vincular este dispositivo.

journal.cli.sol.link.status.error = { $solstone_link_status }: error: { $error }{ $line_break }

journal.cli.sol.link.status.help = uso: { $solstone_link_status_h_label_l }{ $line_break }{ $line_break }Mostrar el estado del enlace y la versión observada del diario remoto.{ $line_break }{ $line_break }opciones:{ $line_break }{ $h_help }{ $space_5 }mostrar este mensaje de ayuda y salir{ $line_break }{ $label_label }{ $space_2 }Etiqueta del paquete de vinculación (predeterminado: el único enlace vinculado){ $line_break }

journal.cli.sol.link.status.label = Etiqueta: { $label }{ $line_break }

journal.cli.sol.link.status.live = Estado: { $state }{ $line_break }

journal.cli.sol.link.status.persist.warning = Advertencia: puede que los cambios de la conexión no se hayan guardado. conserva tus credenciales; el estado anterior puede seguir en el disco.{ $line_break }

journal.cli.sol.link.status.stopped = Estado: detenido{ $line_break }

journal.cli.sol.link.status.usage.error = uso: { $solstone_link_status_h_label_l }{ $line_break }{ $solstone_link_status }: error: { $error }{ $line_break }

journal.cli.sol.link.status.version.fresh = Versión del diario: { $version }{ $line_break }

journal.cli.sol.link.status.version.last = Versión del diario: { $version } (última conocida){ $line_break }

journal.cli.sol.link.status.version.unknown = Versión del diario: desconocida{ $line_break }

journal.cli.sol.link.transport.credential = el material de credenciales del enlace no es válido. vuelve a ejecutar { $solstone_link_join } para este dispositivo.

journal.cli.sol.link.transport.enroll.rejected = La inscripción en el relé se rechazó con { $http } { $status }. Vuelve a ejecutar { $solstone_link_join } si la atestación del paquete está desactualizada.

journal.cli.sol.link.transport.enrollment = Falló la credencial del enlace o la inscripción en el relé. Vuelve a ejecutar { $solstone_link_join } si volver a intentarlo no lo resuelve.

journal.cli.sol.link.transport.framing = falló el encuadre del flujo { $spl } mientras servía. Vuelve a intentarlo; vuelve a vincular si continúa.

journal.cli.sol.link.transport.http = No se pudo interpretar la respuesta del diario por el enlace. Actualiza ambos extremos o vuelve a intentarlo.

journal.cli.sol.link.transport.io = Falló la E/S del transporte del enlace mientras servía. Revisa que el diario esté accesible en { $lan_vpn } o por el relé, y vuelve a intentarlo.

journal.cli.sol.link.transport.json = No se pudo interpretar el { $json } del relé o del puente. Revisa el { $url } del relé y vuelve a intentarlo.

journal.cli.sol.link.transport.no.endpoint = No hay un punto de conexión del diario disponible. Vuelve a ejecutar { $solstone_link_join } o pasa { $relay_url }, a menos que uses { $direct } a propósito.

journal.cli.sol.link.transport.not.paired = Faltan las credenciales de vinculación. Ejecuta { $solstone_link_join } antes de { $solstone_link_serve }.

journal.cli.sol.link.transport.offset = Falló la búsqueda del desfase local. Revisa el reloj del sistema y vuelve a intentarlo.

journal.cli.sol.link.transport.pair.data = los datos de vinculación guardados no son válidos. vuelve a ejecutar { $solstone_link_join } para este dispositivo.

journal.cli.sol.link.transport.refresh.rejected = el relé rechazó la renovación del token con { $http } { $status }. vuelve a ejecutar { $solstone_link_join } para este dispositivo.

journal.cli.sol.link.transport.rejected = El diario vinculado rechazó la solicitud de enlace con { $http } { $status }.

journal.cli.sol.link.transport.tls = Falló el saludo seguro del enlace. Vuelve a ejecutar { $solstone_link_join } si cambió el certificado del diario o la vinculación.

journal.cli.sol.link.unpaired.instructions = Este dispositivo no puede conectar con tu diario con la vinculación guardada. Para volver a vincularlo, detén '{ $solstone_link_serve }', quita la carpeta { $path }, ejecuta '{ $solstone_link_join_code_new_pa }' y luego vuelve a iniciar '{ $solstone_link_serve }'.{ $line_break }

journal.cli.sol.moved = '{ $command }' se movió a '{ $journal_command } { $command }'; ejecútalo en su lugar.{ $line_break }(los comandos que ejecutan el diario en esta computadora están bajo '{ $journal_command }'; consulta '{ $journal_help }'.)

journal.cli.sol.moved.navigate = Se movió a `{ $solstone_journal_navigate }`: ejecútalo en su lugar.{ $line_break }

journal.cli.sol.profile.cadence = Cadencia:

journal.cli.sol.profile.description = descripción: { $value }

journal.cli.sol.profile.detached = facetas desacopladas: { $detached_label }

journal.cli.sol.profile.entity.id = id de entidad: { $value }

journal.cli.sol.profile.full.interactions = { $space_2 }recuento de interacciones recientes en 30 días: { $value }

journal.cli.sol.profile.full.interval = { $space_2 }intervalo promedio en días: { $value }

journal.cli.sol.profile.full.last.seen = { $space_2 }última vez que se vio: { $value }

journal.cli.sol.profile.full.quiet = { $space_2 }en silencio desde: { $value }

journal.cli.sol.profile.identity = { $name } · { $kind } · facets={ $facets } · self={ $self_value } · hidden={ $hidden }

journal.cli.sol.profile.interactions = recuento de interacciones recientes en 30 días: { $value }

journal.cli.sol.profile.interval = intervalo promedio en días: { $value }

journal.cli.sol.profile.last.seen = última vez que se vio: { $value }

journal.cli.sol.profile.name = nombre: { $value }

journal.cli.sol.profile.not.found = no se encontró el perfil: { $name }

journal.cli.sol.profile.quiet = en silencio desde: { $value }

journal.cli.sol.profile.type = tipo: { $value }

journal.cli.sol.resident.platform = las señales de apagado no están disponibles en esta plataforma

journal.cli.sol.resident.signal = falló la configuración de la señal del residente nativo { $solstone_name }: { $error }

journal.cli.sol.resident.startup = falló el vaciado de inicio del residente nativo { $solstone_name }: { $error }

journal.cli.sol.root.inspect = falló la resolución nativa de la raíz del proyecto de { $solstone_name }: no se pudo inspeccionar el ejecutable actual: { $error }

journal.cli.sol.root.package = falló la resolución nativa de la raíz del proyecto de { $solstone_name }: no se pudo encontrar la copia de trabajo de origen ni el paquete { $solstone_name } instalado desde { $path }

journal.cli.sol.skills.agent.invalid = { $skill_command }: error: { $agent_flag } { $agent } no compatible

journal.cli.sol.skills.agent.required = { $skill_command }: error: { $agent_flag } requiere un valor

journal.cli.sol.skills.argument.invalid = { $skill_command }: error: argumento no reconocido { $argument }

journal.cli.sol.skills.bundle.linked = el skill empaquetado contiene elementos enlazados o especiales

journal.cli.sol.skills.cleanup.failed = refrescar los skills del usuario: se instaló { $path }; no se pudo limpiar el skill anterior en { $backup }: { $error }

journal.cli.sol.skills.config.decode = error al decodificar la configuración del diario: { $kind }

journal.cli.sol.skills.config.not.directory = { $path } no es un directorio

journal.cli.sol.skills.copy.special = no se permiten archivos especiales en los árboles de skills del usuario

journal.cli.sol.skills.copy.symlink = no se permiten enlaces simbólicos en los árboles de skills del usuario

journal.cli.sol.skills.cwd.failed = { $solstone }: el directorio de trabajo de los skills nativos no está disponible: { $error }. Ejecuta este comando desde un directorio accesible.

journal.cli.sol.skills.exe.current = no se puede determinar el ejecutable actual: { $error }

journal.cli.sol.skills.exe.parent = el ejecutable actual no tiene directorio padre

journal.cli.sol.skills.file.utf = { $skill_file } no es { $encoding } válido: { $error }

journal.cli.sol.skills.home.absolute = el directorio de inicio debe ser una ruta absoluta

journal.cli.sol.skills.home.missing = { $solstone }: el directorio de inicio de los skills nativos no está disponible. Define { $home } como tu directorio de inicio.

journal.cli.sol.skills.identity.census = no se pudo leer el censo: { $detail }

journal.cli.sol.skills.identity.home.must.be.absolute = el directorio de inicio debe ser absoluto

journal.cli.sol.skills.identity.installation.identity.is.unsupported.for.android = la identidad de instalación no es compatible con Android

journal.cli.sol.skills.identity.io.clone.namespace.directory = clonar el directorio del espacio de nombres: { $error }

journal.cli.sol.skills.identity.io.clone.namespace.registry = clonar el registro del espacio de nombres: { $error }

journal.cli.sol.skills.identity.io.open.home.directory = abrir el directorio de inicio: { $error }

journal.cli.sol.skills.identity.io.open.namespace.directory = abrir el directorio del espacio de nombres: { $error }

journal.cli.sol.skills.identity.io.open.namespace.registry = abrir el registro del espacio de nombres: { $error }

journal.cli.sol.skills.identity.io.read.namespace.registry = leer el registro del espacio de nombres: { $error }

journal.cli.sol.skills.identity.io.stat.storage.directory = stat del directorio de almacenamiento: { $error }

journal.cli.sol.skills.identity.io.stat.storage.file = stat del archivo de almacenamiento: { $error }

journal.cli.sol.skills.identity.namespace.entry.is.not.lowercase.hex = la entrada del espacio de nombres no es hexadecimal en minúsculas

journal.cli.sol.skills.identity.namespace.registry.entries.limit = el registro del espacio de nombres supera las 1024 entradas

journal.cli.sol.skills.identity.namespace.registry.size.limit = el registro del espacio de nombres supera los 16 MiB

journal.cli.sol.skills.identity.path.escapes.its.absolute.root = la ruta se sale de su raíz absoluta

journal.cli.sol.skills.identity.path.must.be.absolute.and.nul.free = la ruta debe ser absoluta y sin NUL

journal.cli.sol.skills.identity.root = no se puede derivar el token raíz de { $path }: { $detail }

journal.cli.sol.skills.identity.storage.directory.does.not.have.private.mode = el directorio de almacenamiento no tiene el modo { $mode }

journal.cli.sol.skills.identity.storage.directory.is.not.owned.by.the.current.user = el directorio de almacenamiento no es propiedad del usuario actual

journal.cli.sol.skills.identity.storage.file.does.not.have.private.mode = el archivo de almacenamiento no tiene el modo { $mode }

journal.cli.sol.skills.identity.storage.file.is.not.owned.by.the.current.user = el archivo de almacenamiento no es propiedad del usuario actual

journal.cli.sol.skills.install.done = se instaló { $agent } { $skill } -> { $path }

journal.cli.sol.skills.journal.absolute = el diario seleccionado debe ser una ruta absoluta

journal.cli.sol.skills.journal.resolved = la ruta resuelta del diario no es absoluta

journal.cli.sol.skills.operation.error = error: { $operation } { $path }: { $detail }

journal.cli.sol.skills.owner.base = no se puede formar la base del propietario en { $path }: { $detail }

journal.cli.sol.skills.path.absolute = la ruta debe ser absoluta

journal.cli.sol.skills.path.ancestor.absent = no se encontró un ancestro existente para { $path }

journal.cli.sol.skills.path.canonical = no se pudo canonizar el ancestro { $path }: { $error }

journal.cli.sol.skills.path.inspect = no se pudo inspeccionar el ancestro { $path }: { $error }

journal.cli.sol.skills.path.overlap = la ruta { $path } se superpone con el diario protegido

journal.cli.sol.skills.path.resolved.overlap = la ruta resuelta del ancestro { $path } se superpone con el diario protegido

journal.cli.sol.skills.payload.failed = { $solstone }: la raíz de la carga de los skills nativos no está disponible: { $detail }. Vuelve a instalar { $solstone } y { $core }.

journal.cli.sol.skills.payload.project = error: se esperaba el skill del proyecto en { $path }

journal.cli.sol.skills.payload.umbrella = error: se esperaba el skill paraguas empaquetado en { $skill_file } ({ $path })

journal.cli.sol.skills.project.agent = error: { $agent_flag } { $agent } no es compatible con { $project_flag }; usa { $all_option } o { $claude_option }

journal.cli.sol.skills.project.missing = se esperaba el skill del proyecto en { $path }

journal.cli.sol.skills.publish.rollback = no se pudo publicar el skill: { $error }; no se pudo restaurar: { $restore }; ruta de la copia de seguridad: { $path }

journal.cli.sol.skills.registry.unknown = el registro de instalación es desconocido

journal.cli.sol.skills.remove.directory = se rechaza quitar lo que no es un directorio

journal.cli.sol.skills.remove.done = se quitó { $agent } { $skill } -> { $path }

journal.cli.sol.skills.remove.retired = se quitó { $agent } { $skill } (skill de producto retirado) -> { $path }

journal.cli.sol.skills.remove.stale = se quitó { $agent } { $skill } (desactualizado) -> { $path }

journal.cli.sol.skills.remove.symlink = se rechaza quitar lo que no es un enlace simbólico

journal.cli.sol.skills.replace.done = se reemplazó { $agent } { $skill } -> { $path }

journal.cli.sol.skills.skip.absent = se omitió { $agent } { $skill } (nada que quitar)

journal.cli.sol.skills.skip.all = no se encontraron directorios de configuración de agentes de código de IA; se omite el registro del skill

journal.cli.sol.skills.skip.config = se omitió { $agent } (directorio de configuración ausente en { $path })

journal.cli.sol.skills.status.absent = { $agent } { $skill } no instalado

journal.cli.sol.skills.status.header = agente     skill                estado

journal.cli.sol.skills.status.installed = { $agent } { $skill } instalado

journal.cli.sol.skills.subcommand.invalid = { $skill_command }: error: subcomando desconocido { $command_name }

journal.cli.sol.skills.subcommand.missing = { $skill_command }: error: falta el subcomando

journal.cli.sol.skills.symlink.unsupported = no se admite crear enlaces simbólicos

journal.cli.sol.skills.target.ineligible = el destino no es elegible para la instalación

journal.cli.sol.skills.target.parent = el destino no tiene directorio padre

journal.cli.sol.skills.target.preserved = la ruta de destino se superpone con el diario protegido o se conserva

journal.cli.sol.skills.tree.directory = el árbol de skills requiere un directorio ordinario

journal.cli.sol.skills.tree.linked = no se permiten elementos enlazados en los árboles de skills del usuario

journal.cli.sol.skills.tree.reference = la referencia del skill empaquetado requiere un directorio con un { $skill_file } normal

journal.cli.sol.skills.tree.special = no se permiten elementos especiales en los árboles de skills del usuario

journal.cli.sol.skills.usage = Uso: { $syntax }

journal.cli.sol.skills.warning.existing = advertencia { $agent } { $skill } -> { $path } (se conserva el contenido existente en el destino)

journal.cli.sol.skills.warning.retired = advertencia { $agent } { $skill } -> { $path } (no coincide con el skill retirado { $retired_skill }; se conserva el contenido existente en { $path })

journal.cli.sol.skills.warning.stale = advertencia { $agent } { $skill } -> { $path } (se conserva el contenido existente en el destino desactualizado)

journal.cli.sol.skills.warnings = Advertencias:

journal.cli.sol.speakers.attribute.segment.accumulated.voiceprints = { $line_break }Huellas de voz acumuladas:

journal.cli.sol.speakers.attribute.segment.by.method = { $line_break }Por método:

journal.cli.sol.speakers.attribute.segment.embeddings = { $quantity ->
    [one] { $spacing_2 }{ $entity_id }: { $embeddings } vector
    [many] { $spacing_2 }{ $entity_id }: { $embeddings } vectores
   *[other] { $spacing_2 }{ $entity_id }: { $embeddings } vectores
    }

journal.cli.sol.speakers.attribute.segment.resolved = Resuelto:{ $spacing_2 }{ $resolved }

journal.cli.sol.speakers.attribute.segment.sentences = Oraciones: { $count }

journal.cli.sol.speakers.attribute.segment.unmatched = Sin coincidencia: { $count }

journal.cli.sol.speakers.attribute.segment.wrote = { $line_break }Se escribió: { $path }

journal.cli.sol.speakers.backfill.completed.count = Cantidad completada:{ $spacing_11 }{ $count }

journal.cli.sol.speakers.backfill.elapsed.s = Transcurrido:{ $spacing_19 }{ $elapsed } s

journal.cli.sol.speakers.backfill.error.count = Cantidad de errores:{ $spacing_15 }{ $count }

journal.cli.sol.speakers.backfill.errors.and.more = { $spacing_2 }... y { $count } más

journal.cli.sol.speakers.backfill.failure.detail = Detalle del fallo:{ $spacing_12 }{ $detail }

journal.cli.sol.speakers.backfill.failure.stage = Etapa del fallo:{ $spacing_13 }{ $stage }

journal.cli.sol.speakers.backfill.last.seen.entities.seen = Entidades vistas:{ $spacing_12 }{ $count }

journal.cli.sol.speakers.backfill.last.seen.pending.by.entity = { $line_break }Pendientes por entidad:

journal.cli.sol.speakers.backfill.last.seen.rows.pending = Filas pendientes:{ $spacing_13 }{ $count }

journal.cli.sol.speakers.backfill.last.seen.rows.written = Filas escritas:{ $spacing_13 }{ $count }

journal.cli.sol.speakers.backfill.last.seen.speaker.label.files.read = Archivos de etiquetas de hablante leídos: { $count }

journal.cli.sol.speakers.backfill.last.seen.voiceprint.rows.scanned = Filas de huellas de voz escaneadas:{ $spacing_2 }{ $count }

journal.cli.sol.speakers.backfill.note.backfill.operation.has.immutable = Nota: la operación de relleno { $id } tiene flags inmutables.{ $line_break }Para reanudar con una configuración compatible, prueba:{ $line_break }{ $spacing_2 }{ $command } { $option_operation_id } { $id } { $option_commit }{ $line_break }

journal.cli.sol.speakers.backfill.operation.id = ID de operación:{ $spacing_14 }{ $op_id }

journal.cli.sol.speakers.backfill.pending.count = Cantidad pendiente:{ $spacing_13 }{ $count }

journal.cli.sol.speakers.backfill.protected.skipped = Protegidos (omitidos):{ $spacing_7 }{ $count }

journal.cli.sol.speakers.backfill.selected.for.backfill = Seleccionados para el relleno:{ $spacing_5 }{ $count }

journal.cli.sol.speakers.backfill.starting.speaker.attribution.backfill = Iniciando el relleno de atribución de hablantes...

journal.cli.sol.speakers.backfill.status = Estado:{ $spacing_20 }{ $status }

journal.cli.sol.speakers.backfill.total.planned.count = Cantidad total planeada:{ $spacing_7 }{ $count }

journal.cli.sol.speakers.backfill.total.segments.scanned = Total de segmentos escaneados:{ $spacing_4 }{ $count }

journal.cli.sol.speakers.bootstrap.bootstrapping.voiceprints.from.single.speaker = Inicializando huellas de voz a partir de segmentos de un solo hablante...

journal.cli.sol.speakers.bootstrap.stats.embeddings.saved = Vectores guardados: { $count }

journal.cli.sol.speakers.bootstrap.stats.embeddings.skipped.duplicate = Vectores omitidos (duplicado): { $count }

journal.cli.sol.speakers.bootstrap.stats.embeddings.skipped.owner = Vectores omitidos (propietario): { $count }

journal.cli.sol.speakers.bootstrap.stats.entities.created = Entidades creadas: { $count }

journal.cli.sol.speakers.bootstrap.stats.segments.scanned = Segmentos escaneados: { $count }

journal.cli.sol.speakers.bootstrap.stats.single.speaker.segments = Segmentos de un solo hablante: { $count }

journal.cli.sol.speakers.bootstrap.stats.top.speakers.by.embedding.count = Hablantes principales por cantidad de vectores:

journal.cli.sol.speakers.bootstrap.stats.unique.speakers = Hablantes únicos: { $count }

journal.cli.sol.speakers.build.from.tags.can.build.from.tags = Se puede construir a partir de etiquetas: { $available }

journal.cli.sol.speakers.build.from.tags.guidance = Orientación: { $guidance }

journal.cli.sol.speakers.build.from.tags.manual.tags = Etiquetas manuales: { $tags }

journal.cli.sol.speakers.build.from.tags.next.step = Siguiente paso: { $step }

journal.cli.sol.speakers.build.from.tags.observed = Observado: { $observed }

journal.cli.sol.speakers.build.from.tags.owner.centroid.already.confirmed = El centroide del propietario ya está confirmado.

journal.cli.sol.speakers.build.from.tags.owner.centroid.confirmed.from.manual = Centroide del propietario confirmado a partir de etiquetas manuales (principal: { $principal }, cluster_size: { $cluster_size })

journal.cli.sol.speakers.build.from.tags.owner.manual.tags.are.not = Las etiquetas manuales del propietario no están listas.

journal.cli.sol.speakers.build.from.tags.reason = Motivo: { $reason }

journal.cli.sol.speakers.build.from.tags.threshold = Umbral: { $threshold }

journal.cli.sol.speakers.confirm.owner.review.error.owner.voice.candidates.must = error: los candidatos de voz del propietario se deben confirmar en el navegador.{ $line_break }Revisa y confirma en { $review_entry }

journal.cli.sol.speakers.correct.already.correct = Ya es correcto: { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }

journal.cli.sol.speakers.correct.corrected = Se corrigió { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }: { $old } -> { $new_speaker }

journal.cli.sol.speakers.correct.preview.with.solstone.call.speakers = Vista previa con: { $command } { $old_speaker } { $new_speaker }

journal.cli.sol.speakers.correct.propagation.available.statements.in.segments = { $quantity ->
    [one] { $segments_quantity ->
    [one] Propagación disponible: cambiaría { $statements } enunciado en { $segments } segmento
    [many] Propagación disponible: cambiaría { $statements } enunciado en { $segments } segmentos
   *[other] Propagación disponible: cambiaría { $statements } enunciado en { $segments } segmentos
    }
    [many] { $segments_quantity ->
    [one] Propagación disponible: cambiarían { $statements } enunciados en { $segments } segmento
    [many] Propagación disponible: cambiarían { $statements } enunciados en { $segments } segmentos
   *[other] Propagación disponible: cambiarían { $statements } enunciados en { $segments } segmentos
    }
   *[other] { $segments_quantity ->
    [one] Propagación disponible: cambiarían { $statements } enunciados en { $segments } segmento
    [many] Propagación disponible: cambiarían { $statements } enunciados en { $segments } segmentos
   *[other] Propagación disponible: cambiarían { $statements } enunciados en { $segments } segmentos
    }
    }

journal.cli.sol.speakers.correct.propagation.nothing.else.would.change = Propagación: no cambiaría nada más

journal.cli.sol.speakers.correct.unassigned.unassigned = sin asignar

journal.cli.sol.speakers.correct.voiceprint.removal = Retiro de huella de voz: { $outcome }

journal.cli.sol.speakers.day.segments.showing.of.segments.limit = { $quantity ->
    [one] Se muestra { $shown } de { $total } segmento (límite { $limit })
    [many] Se muestran { $shown } de { $total } segmentos (límite { $limit })
   *[other] Se muestran { $shown } de { $total } segmentos (límite { $limit })
    }

journal.cli.sol.speakers.discover.cluster.samples.across.segments = { $quantity ->
    [one] { $segments_quantity ->
    [one] { $spacing_2 }Clúster { $cluster }: { $samples } muestra a través de { $segments } segmento
    [many] { $spacing_2 }Clúster { $cluster }: { $samples } muestra a través de { $segments } segmentos
   *[other] { $spacing_2 }Clúster { $cluster }: { $samples } muestra a través de { $segments } segmentos
    }
    [many] { $segments_quantity ->
    [one] { $spacing_2 }Clúster { $cluster }: { $samples } muestras a través de { $segments } segmento
    [many] { $spacing_2 }Clúster { $cluster }: { $samples } muestras a través de { $segments } segmentos
   *[other] { $spacing_2 }Clúster { $cluster }: { $samples } muestras a través de { $segments } segmentos
    }
   *[other] { $segments_quantity ->
    [one] { $spacing_2 }Clúster { $cluster }: { $samples } muestras a través de { $segments } segmento
    [many] { $spacing_2 }Clúster { $cluster }: { $samples } muestras a través de { $segments } segmentos
   *[other] { $spacing_2 }Clúster { $cluster }: { $samples } muestras a través de { $segments } segmentos
    }
    }

journal.cli.sol.speakers.discover.found.unknown.speaker.cluster.s = { $quantity ->
    [one] Se encontró { $clusters } clúster de hablante desconocido:{ $line_break }
    [many] Se encontraron { $clusters } clústeres de hablante desconocido:{ $line_break }
   *[other] Se encontraron { $clusters } clústeres de hablante desconocido:{ $line_break }
    }

journal.cli.sol.speakers.discover.no.recurring.unknown.speakers.found = No se encontraron hablantes desconocidos recurrentes.

journal.cli.sol.speakers.discover.sid = { $spacing_4 }- { $day }/{ $stream }/{ $segment } sid={ $sentence_id }: { $text }

journal.cli.sol.speakers.discovery.error.try.again = vuelve a intentarlo

journal.cli.sol.speakers.discovery.warnings.warning = Advertencia: { $reason_code }: { $message }

journal.cli.sol.speakers.discovery.warnings.warning.count = Advertencia: { $reason_code }: { $message } count={ $count }

journal.cli.sol.speakers.errors.errors = { $line_break }Errores ({ $count }):

journal.cli.sol.speakers.follow.backfill.backfill.in.progress.status.completed = Relleno en curso: status={ $status_kind }, completed={ $completed }/{ $total }, pending={ $pending }

journal.cli.sol.speakers.follow.backfill.backfill.not.completed.status.to = { $line_break }El relleno no se completó (estado: { $status_kind }).{ $line_break }Para revisar el estado:{ $line_break }{ $spacing_2 }{ $command_1 } { $operation_id }{ $line_break }Para reanudar:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.follow.repair.start.repair.in.progress.segments.repaired = Reparación en curso, segmentos reparados hasta ahora: { $count }

journal.cli.sol.speakers.follow.repair.start.repair.running = Reparación en ejecución: { $operation_id }

journal.cli.sol.speakers.identify.error.inspect.operations.with.solstone.call = Inspecciona las operaciones con: { $command }

journal.cli.sol.speakers.identify.error.inspect.this.operation.with.solstone = Inspecciona esta operación con: { $command } { $operation_id }

journal.cli.sol.speakers.identify.error.retry.with.the.same.request = Vuelve a intentarlo con el mismo { $option_request_id } { $request_id }.

journal.cli.sol.speakers.identify.name.required.usage = Uso: { $command } { $syntax }{ $line_break }Prueba '{ $help_command }' para obtener ayuda.{ $line_break }{ $box_left } Error { $box_top }{ $line_break }{ $box_edge } Valor no válido: el nombre o { $entity_option } es obligatorio{ $box_padding }{ $box_edge }{ $line_break }{ $box_bottom }{ $line_break }

journal.cli.sol.speakers.parse.stream.layout.option.error.invalid.value.for.stream = Error: valor no válido para { $option_stream_layout }

journal.cli.sol.speakers.presence.candidate.meeting.unvoiced.screen.meeting.days.voice.no = { $spacing_2 }{ $name } ({ $entity }): screen={ $first_count }, meeting_days={ $second_count }, voice=no

journal.cli.sol.speakers.presence.candidate.meeting.voiced.screen.meeting.days.voice.yes = { $spacing_2 }{ $name } ({ $entity }): screen={ $first_count }, meeting_days={ $second_count }, voice=yes

journal.cli.sol.speakers.presence.candidate.mention.unvoiced.setting.speakers.voice.no = { $spacing_2 }{ $name } ({ $entity }): setting={ $first_count }, speakers={ $second_count }, voice=no

journal.cli.sol.speakers.presence.candidate.mention.voiced.setting.speakers.voice.yes = { $spacing_2 }{ $name } ({ $entity }): setting={ $first_count }, speakers={ $second_count }, voice=yes

journal.cli.sol.speakers.presence.cluster.statements.segments.conversations = { $quantity ->
    [one] { $segments_quantity ->
    [one] { $conversations_quantity ->
    [one] Clúster { $cluster }: { $statements } enunciado, { $segments } segmento, { $conversations } conversación
    [many] Clúster { $cluster }: { $statements } enunciado, { $segments } segmento, { $conversations } conversaciones
   *[other] Clúster { $cluster }: { $statements } enunciado, { $segments } segmento, { $conversations } conversaciones
    }
    [many] { $conversations_quantity ->
    [one] Clúster { $cluster }: { $statements } enunciado, { $segments } segmentos, { $conversations } conversación
    [many] Clúster { $cluster }: { $statements } enunciado, { $segments } segmentos, { $conversations } conversaciones
   *[other] Clúster { $cluster }: { $statements } enunciado, { $segments } segmentos, { $conversations } conversaciones
    }
   *[other] { $conversations_quantity ->
    [one] Clúster { $cluster }: { $statements } enunciado, { $segments } segmentos, { $conversations } conversación
    [many] Clúster { $cluster }: { $statements } enunciado, { $segments } segmentos, { $conversations } conversaciones
   *[other] Clúster { $cluster }: { $statements } enunciado, { $segments } segmentos, { $conversations } conversaciones
    }
    }
    [many] { $segments_quantity ->
    [one] { $conversations_quantity ->
    [one] Clúster { $cluster }: { $statements } enunciados, { $segments } segmento, { $conversations } conversación
    [many] Clúster { $cluster }: { $statements } enunciados, { $segments } segmento, { $conversations } conversaciones
   *[other] Clúster { $cluster }: { $statements } enunciados, { $segments } segmento, { $conversations } conversaciones
    }
    [many] { $conversations_quantity ->
    [one] Clúster { $cluster }: { $statements } enunciados, { $segments } segmentos, { $conversations } conversación
    [many] Clúster { $cluster }: { $statements } enunciados, { $segments } segmentos, { $conversations } conversaciones
   *[other] Clúster { $cluster }: { $statements } enunciados, { $segments } segmentos, { $conversations } conversaciones
    }
   *[other] { $conversations_quantity ->
    [one] Clúster { $cluster }: { $statements } enunciados, { $segments } segmentos, { $conversations } conversación
    [many] Clúster { $cluster }: { $statements } enunciados, { $segments } segmentos, { $conversations } conversaciones
   *[other] Clúster { $cluster }: { $statements } enunciados, { $segments } segmentos, { $conversations } conversaciones
    }
    }
   *[other] { $segments_quantity ->
    [one] { $conversations_quantity ->
    [one] Clúster { $cluster }: { $statements } enunciados, { $segments } segmento, { $conversations } conversación
    [many] Clúster { $cluster }: { $statements } enunciados, { $segments } segmento, { $conversations } conversaciones
   *[other] Clúster { $cluster }: { $statements } enunciados, { $segments } segmento, { $conversations } conversaciones
    }
    [many] { $conversations_quantity ->
    [one] Clúster { $cluster }: { $statements } enunciados, { $segments } segmentos, { $conversations } conversación
    [many] Clúster { $cluster }: { $statements } enunciados, { $segments } segmentos, { $conversations } conversaciones
   *[other] Clúster { $cluster }: { $statements } enunciados, { $segments } segmentos, { $conversations } conversaciones
    }
   *[other] { $conversations_quantity ->
    [one] Clúster { $cluster }: { $statements } enunciados, { $segments } segmentos, { $conversations } conversación
    [many] Clúster { $cluster }: { $statements } enunciados, { $segments } segmentos, { $conversations } conversaciones
   *[other] Clúster { $cluster }: { $statements } enunciados, { $segments } segmentos, { $conversations } conversaciones
    }
    }
    }

journal.cli.sol.speakers.presence.cluster.was.not.found.run = No se encontró el clúster { $cluster_id }.{ $line_break }Ejecuta '{ $command }' para producir ids de clúster válidos.{ $line_break }

journal.cli.sol.speakers.presence.co.presence = { $line_break }Copresencia:

journal.cli.sol.speakers.presence.evidence.complete = Evidencia: completa

journal.cli.sol.speakers.presence.evidence.gap.s = { $quantity ->
    [one] Evidencia: { $gaps } hueco
    [many] Evidencia: { $gaps } huecos
   *[other] Evidencia: { $gaps } huecos
    }

journal.cli.sol.speakers.presence.mentions = { $line_break }Menciones:

journal.cli.sol.speakers.presence.no.candidate.entities.found = No se encontraron entidades candidatas.

journal.cli.sol.speakers.propagate.applied.applied.statements.in.segments = { $quantity ->
    [one] { $segment_count_quantity ->
    [one] Se aplicó: { $statement_count } enunciado en { $segment_count } segmento
    [many] Se aplicó: { $statement_count } enunciado en { $segment_count } segmentos
   *[other] Se aplicó: { $statement_count } enunciado en { $segment_count } segmentos
    }
    [many] { $segment_count_quantity ->
    [one] Se aplicaron: { $statement_count } enunciados en { $segment_count } segmento
    [many] Se aplicaron: { $statement_count } enunciados en { $segment_count } segmentos
   *[other] Se aplicaron: { $statement_count } enunciados en { $segment_count } segmentos
    }
   *[other] { $segment_count_quantity ->
    [one] Se aplicaron: { $statement_count } enunciados en { $segment_count } segmento
    [many] Se aplicaron: { $statement_count } enunciados en { $segment_count } segmentos
   *[other] Se aplicaron: { $statement_count } enunciados en { $segment_count } segmentos
    }
    }

journal.cli.sol.speakers.propagate.errors = { $line_break }Errores:

journal.cli.sol.speakers.propagate.nothing.else.would.change = No cambiaría nada más.

journal.cli.sol.speakers.propagate.reverse.with.solstone.call.commit = Revierte con: { $command } { $operation } { $old_speaker } { $new_speaker } { $option_commit }

journal.cli.sol.speakers.propagate.would.change.would.change.statements.in.segments = { $quantity ->
    [one] { $segment_count_quantity ->
    [one] Cambiaría: { $statement_count } enunciado en { $segment_count } segmento
    [many] Cambiaría: { $statement_count } enunciado en { $segment_count } segmentos
   *[other] Cambiaría: { $statement_count } enunciado en { $segment_count } segmentos
    }
    [many] { $segment_count_quantity ->
    [one] Cambiarían: { $statement_count } enunciados en { $segment_count } segmento
    [many] Cambiarían: { $statement_count } enunciados en { $segment_count } segmentos
   *[other] Cambiarían: { $statement_count } enunciados en { $segment_count } segmentos
    }
   *[other] { $segment_count_quantity ->
    [one] Cambiarían: { $statement_count } enunciados en { $segment_count } segmento
    [many] Cambiarían: { $statement_count } enunciados en { $segment_count } segmentos
   *[other] Cambiarían: { $statement_count } enunciados en { $segment_count } segmentos
    }
    }

journal.cli.sol.speakers.rebuild.owner.guidance = Orientación: { $step }

journal.cli.sol.speakers.rebuild.owner.next.step = Siguiente paso: { $next_step }

journal.cli.sol.speakers.rebuild.owner.override.applied.true = Anulación aplicada: true

journal.cli.sol.speakers.rebuild.owner.owner.centroid.rebuild.did.not = La reconstrucción del centroide del propietario no escribió.

journal.cli.sol.speakers.rebuild.owner.owner.centroid.rebuilt.principal.cluster = Centroide del propietario reconstruido (principal: { $principal }, cluster_size: { $cluster_size })

journal.cli.sol.speakers.rebuild.owner.owner.centroid.unchanged.evidence.already = Centroide del propietario sin cambios; la evidencia ya coincide.

journal.cli.sol.speakers.reject.owner.error.version.is.required = Error: { $option_version } es obligatorio.

journal.cli.sol.speakers.repair.check.status.advice.to.check.status.solstone.call = { $line_break }Para revisar el estado:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.repair.dry.run.complete = Completo:{ $spacing_2 }{ $complete }

journal.cli.sol.speakers.repair.dry.run.entities.with.voiceprints.to.remove = { $line_break }Entidades con huellas de voz por quitar: { $entities }

journal.cli.sol.speakers.repair.dry.run.gaps.detected = { $line_break }Huecos detectados ({ $gaps }):

journal.cli.sol.speakers.repair.dry.run.mode.dry.run = Modo:{ $spacing_6 }prueba en seco

journal.cli.sol.speakers.repair.dry.run.rows = { $quantity ->
    [one] { $spacing_2 }- { $id } ({ $rows } fila)
    [many] { $spacing_2 }- { $id } ({ $rows } filas)
   *[other] { $spacing_2 }- { $id } ({ $rows } filas)
    }

journal.cli.sol.speakers.repair.dry.run.segments.to.repair = { $line_break }Segmentos por reparar: { $segments }

journal.cli.sol.speakers.repair.in.progress.status.advice = { $line_break }La reparación puede seguir en ejecución en el diario. No inicies otra.{ $line_break }Para revisar el estado:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.repair.starting.speaker.contamination.repair = Iniciando la reparación de contaminación de hablantes...

journal.cli.sol.speakers.repair.status.clean = Limpio:{ $spacing_5 }{ $clean }

journal.cli.sol.speakers.repair.status.last.failure = Último fallo: { $stage } ({ $code })

journal.cli.sol.speakers.repair.status.segments.repaired = Segmentos reparados: { $count }

journal.cli.sol.speakers.repair.status.status = Estado:{ $spacing_4 }{ $status }

journal.cli.sol.speakers.repair.stopped.advice.to.resume.solstone.call.speakers = { $line_break }Para reanudar:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.report.only = SOLO INFORME: pasa { $option_commit } para persistir.{ $line_break }

journal.cli.sol.speakers.resolve.names.ambiguous = { $line_break }Ambiguos ({ $count }):

journal.cli.sol.speakers.resolve.names.auto.merged = { $line_break }Fusionados automáticamente ({ $count }):

journal.cli.sol.speakers.resolve.names.entities.with.voiceprints = Entidades con huellas de voz: { $count }

journal.cli.sol.speakers.resolve.names.high.similarity.pairs = Pares de similitud alta: { $count }

journal.cli.sol.speakers.resolve.names.merging.name.variants.can.t = Fusionar variantes de nombre no se puede deshacer. Revisa el informe sin { $option_commit } y, cuando el propietario lo apruebe, vuelve a ejecutarlo con { $option_commit } { $option_yes }.

journal.cli.sol.speakers.resolve.names.pairs.compared = Pares comparados: { $count }

journal.cli.sol.speakers.resolve.names.resolving.speaker.name.variants = Resolviendo variantes de nombres de hablantes...

journal.cli.sol.speakers.seed.from.imports.seeding.voiceprints.from.import.segments = Sembrando huellas de voz a partir de segmentos importados...

journal.cli.sol.speakers.seed.from.imports.segments.with.speakers = Segmentos con hablantes: { $count }

journal.cli.sol.speakers.seed.from.imports.speakers.by.embedding.count = Hablantes por cantidad de vectores:

journal.cli.sol.speakers.seed.from.imports.unmatched.speakers = { $line_break }Hablantes sin coincidencia ({ $count }):

journal.cli.sol.speakers.sentences.sentences.for = Oraciones para { $day }/{ $stream }/{ $segment }/{ $source }:

journal.cli.sol.speakers.tag.owner.owner.sentence.already.tagged = La oración del propietario ya está etiquetada: { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }

journal.cli.sol.speakers.tag.owner.tagged.owner.sentence = Se etiquetó la oración del propietario: { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }

journal.cli.sol.speakers.wipe.couldn.t.erase.now.it = no se pudo borrar ahora; espera en la papelera

journal.cli.sol.speakers.wipe.erase.needs.commit = { $option_erase } necesita { $option_commit }

journal.cli.sol.speakers.wipe.erased.erased = borrado

journal.cli.sol.speakers.wipe.files.b = { $quantity ->
    [one] { $label }: { $files } archivo ({ $bytes } B)
    [many] { $label }: { $files } archivos ({ $bytes } B)
   *[other] { $label }: { $files } archivos ({ $bytes } B)
    }

journal.cli.sol.speakers.wipe.in.the.trash = en la papelera

journal.cli.sol.speakers.wipe.in.the.trash.erases = en la papelera; borra { $day }

journal.cli.sol.speakers.wipe.total.files.b = { $quantity ->
    [one] total{ $spacing_14 }: { $files } archivo ({ $bytes } B)
    [many] total{ $spacing_14 }: { $files } archivos ({ $bytes } B)
   *[other] total{ $spacing_14 }: { $files } archivos ({ $bytes } B)
    }

journal.cli.sol.unsupported = Comando nativo de { $solstone_name } no compatible.

journal.cli.sol.update.automatic.choice = no se pudo guardar tu elección.

journal.cli.sol.update.automatic.home = no se pudo encontrar tu directorio de datos.

journal.cli.sol.update.automatic.ineligible = esta instalación no puede actualizarse sola. las actualizaciones automáticas funcionan en una instalación desde { $install_url }.

journal.cli.sol.update.automatic.ineligible.generic = esta instalación no puede actualizarse sola.

journal.cli.sol.update.automatic.linux = { $automatic_command } funciona en linux.

journal.cli.sol.update.automatic.off = las actualizaciones automáticas están desactivadas. para actualizar, ejecuta { $update_command }.

journal.cli.sol.update.automatic.on = las actualizaciones automáticas están activadas. tu diario nunca se reinicia por su cuenta; una versión nueva se inicia la próxima vez que se reinicia.

journal.cli.sol.update.automatic.package.add = se instaló desde un paquete. para recibir actualizaciones con el actualizador de tu sistema, agrega el repositorio de { $brand }: { $package_url }

journal.cli.sol.update.automatic.prefix = el nombre del directorio de instalación no puede ir en un archivo de servicio.

journal.cli.sol.update.automatic.run = { $result ->
    [up-to-date] actualización automática: al día
    [installed] actualización automática: se instaló una versión nueva
    [off] actualización automática: las actualizaciones automáticas están desactivadas
    [paused] actualización automática: se detuvo tras dos intentos que no funcionaron
    [held] actualización automática: las actualizaciones están retenidas
    [hold-unknown] actualización automática: no se pudo leer la retención de la actualización
    [offline] actualización automática: no se pudo conectar con el servicio de actualizaciones
    [metered] actualización automática: esperando una conexión que no es de uso medido
    [waiting-for-signed-installer] actualización automática: esperando el próximo lanzamiento
    [busy] actualización automática: otra actualización estaba en ejecución
    [not-eligible] actualización automática: esta instalación no puede actualizarse sola
    *[failed] actualización automática: la actualización no funcionó ({ $code })
    }

journal.cli.sol.update.automatic.session = no se pudo configurar la búsqueda diaria de actualizaciones. puede que tu sesión de usuario de systemd no esté en ejecución.

journal.cli.sol.update.automatic.status = { $choice ->
    [on] actualizaciones automáticas: activado
    [off] actualizaciones automáticas: desactivado
    [unasked] actualizaciones automáticas: sin activar
    *[paused] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron
    }

journal.cli.sol.update.automatic.status.checked = { $choice ->
    [on] actualizaciones automáticas: activado · última revisión { $date }
    [off] actualizaciones automáticas: desactivado · última revisión { $date }
    [unasked] actualizaciones automáticas: sin activar · última revisión { $date }
    *[paused] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · última revisión { $date }
    }

journal.cli.sol.update.automatic.status.off.result = { $result ->
    [up-to-date] actualizaciones automáticas: desactivado · al día
    [installed] actualizaciones automáticas: desactivado · se instaló una versión nueva
    [off] actualizaciones automáticas: desactivado · las actualizaciones automáticas están desactivadas
    [paused] actualizaciones automáticas: desactivado · se detuvo tras dos intentos que no funcionaron
    [held] actualizaciones automáticas: desactivado · las actualizaciones están retenidas
    [hold-unknown] actualizaciones automáticas: desactivado · no se pudo leer la retención de la actualización
    [offline] actualizaciones automáticas: desactivado · no se pudo conectar con el servicio de actualizaciones
    [metered] actualizaciones automáticas: desactivado · esperando una conexión que no es de uso medido
    [waiting-for-signed-installer] actualizaciones automáticas: desactivado · esperando el próximo lanzamiento
    [busy] actualizaciones automáticas: desactivado · otra actualización estaba en ejecución
    [not-eligible] actualizaciones automáticas: desactivado · esta instalación no puede actualizarse sola
    *[failed] actualizaciones automáticas: desactivado · la actualización no funcionó ({ $code })
    }

journal.cli.sol.update.automatic.status.off.result.checked = { $result ->
    [up-to-date] actualizaciones automáticas: desactivado · última revisión { $date } · al día
    [installed] actualizaciones automáticas: desactivado · última revisión { $date } · se instaló una versión nueva
    [off] actualizaciones automáticas: desactivado · última revisión { $date } · las actualizaciones automáticas están desactivadas
    [paused] actualizaciones automáticas: desactivado · última revisión { $date } · se detuvo tras dos intentos que no funcionaron
    [held] actualizaciones automáticas: desactivado · última revisión { $date } · las actualizaciones están retenidas
    [hold-unknown] actualizaciones automáticas: desactivado · última revisión { $date } · no se pudo leer la retención de la actualización
    [offline] actualizaciones automáticas: desactivado · última revisión { $date } · no se pudo conectar con el servicio de actualizaciones
    [metered] actualizaciones automáticas: desactivado · última revisión { $date } · esperando una conexión que no es de uso medido
    [waiting-for-signed-installer] actualizaciones automáticas: desactivado · última revisión { $date } · esperando el próximo lanzamiento
    [busy] actualizaciones automáticas: desactivado · última revisión { $date } · otra actualización estaba en ejecución
    [not-eligible] actualizaciones automáticas: desactivado · última revisión { $date } · esta instalación no puede actualizarse sola
    *[failed] actualizaciones automáticas: desactivado · última revisión { $date } · la actualización no funcionó ({ $code })
    }

journal.cli.sol.update.automatic.status.on.result = { $result ->
    [up-to-date] actualizaciones automáticas: activado · al día
    [installed] actualizaciones automáticas: activado · se instaló una versión nueva
    [off] actualizaciones automáticas: activado · las actualizaciones automáticas están desactivadas
    [paused] actualizaciones automáticas: activado · se detuvo tras dos intentos que no funcionaron
    [held] actualizaciones automáticas: activado · las actualizaciones están retenidas
    [hold-unknown] actualizaciones automáticas: activado · no se pudo leer la retención de la actualización
    [offline] actualizaciones automáticas: activado · no se pudo conectar con el servicio de actualizaciones
    [metered] actualizaciones automáticas: activado · esperando una conexión que no es de uso medido
    [waiting-for-signed-installer] actualizaciones automáticas: activado · esperando el próximo lanzamiento
    [busy] actualizaciones automáticas: activado · otra actualización estaba en ejecución
    [not-eligible] actualizaciones automáticas: activado · esta instalación no puede actualizarse sola
    *[failed] actualizaciones automáticas: activado · la actualización no funcionó ({ $code })
    }

journal.cli.sol.update.automatic.status.on.result.checked = { $result ->
    [up-to-date] actualizaciones automáticas: activado · última revisión { $date } · al día
    [installed] actualizaciones automáticas: activado · última revisión { $date } · se instaló una versión nueva
    [off] actualizaciones automáticas: activado · última revisión { $date } · las actualizaciones automáticas están desactivadas
    [paused] actualizaciones automáticas: activado · última revisión { $date } · se detuvo tras dos intentos que no funcionaron
    [held] actualizaciones automáticas: activado · última revisión { $date } · las actualizaciones están retenidas
    [hold-unknown] actualizaciones automáticas: activado · última revisión { $date } · no se pudo leer la retención de la actualización
    [offline] actualizaciones automáticas: activado · última revisión { $date } · no se pudo conectar con el servicio de actualizaciones
    [metered] actualizaciones automáticas: activado · última revisión { $date } · esperando una conexión que no es de uso medido
    [waiting-for-signed-installer] actualizaciones automáticas: activado · última revisión { $date } · esperando el próximo lanzamiento
    [busy] actualizaciones automáticas: activado · última revisión { $date } · otra actualización estaba en ejecución
    [not-eligible] actualizaciones automáticas: activado · última revisión { $date } · esta instalación no puede actualizarse sola
    *[failed] actualizaciones automáticas: activado · última revisión { $date } · la actualización no funcionó ({ $code })
    }

journal.cli.sol.update.automatic.status.paused.result = { $result ->
    [up-to-date] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · al día
    [installed] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · se instaló una versión nueva
    [off] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · las actualizaciones automáticas están desactivadas
    [paused] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · se detuvo tras dos intentos que no funcionaron
    [held] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · las actualizaciones están retenidas
    [hold-unknown] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · no se pudo leer la retención de la actualización
    [offline] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · no se pudo conectar con el servicio de actualizaciones
    [metered] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · esperando una conexión que no es de uso medido
    [waiting-for-signed-installer] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · esperando el próximo lanzamiento
    [busy] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · otra actualización estaba en ejecución
    [not-eligible] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · esta instalación no puede actualizarse sola
    *[failed] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · la actualización no funcionó ({ $code })
    }

journal.cli.sol.update.automatic.status.paused.result.checked = { $result ->
    [up-to-date] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · última revisión { $date } · al día
    [installed] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · última revisión { $date } · se instaló una versión nueva
    [off] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · última revisión { $date } · las actualizaciones automáticas están desactivadas
    [paused] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · última revisión { $date } · se detuvo tras dos intentos que no funcionaron
    [held] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · última revisión { $date } · las actualizaciones están retenidas
    [hold-unknown] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · última revisión { $date } · no se pudo leer la retención de la actualización
    [offline] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · última revisión { $date } · no se pudo conectar con el servicio de actualizaciones
    [metered] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · última revisión { $date } · esperando una conexión que no es de uso medido
    [waiting-for-signed-installer] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · última revisión { $date } · esperando el próximo lanzamiento
    [busy] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · última revisión { $date } · otra actualización estaba en ejecución
    [not-eligible] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · última revisión { $date } · esta instalación no puede actualizarse sola
    *[failed] actualizaciones automáticas: se detuvo tras dos intentos que no funcionaron · última revisión { $date } · la actualización no funcionó ({ $code })
    }

journal.cli.sol.update.automatic.status.unasked.result = { $result ->
    [up-to-date] actualizaciones automáticas: sin activar · al día
    [installed] actualizaciones automáticas: sin activar · se instaló una versión nueva
    [off] actualizaciones automáticas: sin activar · las actualizaciones automáticas están desactivadas
    [paused] actualizaciones automáticas: sin activar · se detuvo tras dos intentos que no funcionaron
    [held] actualizaciones automáticas: sin activar · las actualizaciones están retenidas
    [hold-unknown] actualizaciones automáticas: sin activar · no se pudo leer la retención de la actualización
    [offline] actualizaciones automáticas: sin activar · no se pudo conectar con el servicio de actualizaciones
    [metered] actualizaciones automáticas: sin activar · esperando una conexión que no es de uso medido
    [waiting-for-signed-installer] actualizaciones automáticas: sin activar · esperando el próximo lanzamiento
    [busy] actualizaciones automáticas: sin activar · otra actualización estaba en ejecución
    [not-eligible] actualizaciones automáticas: sin activar · esta instalación no puede actualizarse sola
    *[failed] actualizaciones automáticas: sin activar · la actualización no funcionó ({ $code })
    }

journal.cli.sol.update.automatic.status.unasked.result.checked = { $result ->
    [up-to-date] actualizaciones automáticas: sin activar · última revisión { $date } · al día
    [installed] actualizaciones automáticas: sin activar · última revisión { $date } · se instaló una versión nueva
    [off] actualizaciones automáticas: sin activar · última revisión { $date } · las actualizaciones automáticas están desactivadas
    [paused] actualizaciones automáticas: sin activar · última revisión { $date } · se detuvo tras dos intentos que no funcionaron
    [held] actualizaciones automáticas: sin activar · última revisión { $date } · las actualizaciones están retenidas
    [hold-unknown] actualizaciones automáticas: sin activar · última revisión { $date } · no se pudo leer la retención de la actualización
    [offline] actualizaciones automáticas: sin activar · última revisión { $date } · no se pudo conectar con el servicio de actualizaciones
    [metered] actualizaciones automáticas: sin activar · última revisión { $date } · esperando una conexión que no es de uso medido
    [waiting-for-signed-installer] actualizaciones automáticas: sin activar · última revisión { $date } · esperando el próximo lanzamiento
    [busy] actualizaciones automáticas: sin activar · última revisión { $date } · otra actualización estaba en ejecución
    [not-eligible] actualizaciones automáticas: sin activar · última revisión { $date } · esta instalación no puede actualizarse sola
    *[failed] actualizaciones automáticas: sin activar · última revisión { $date } · la actualización no funcionó ({ $code })
    }

journal.cli.sol.update.catalogue.download = no se pudo obtener el catálogo de lanzamientos.

journal.cli.sol.update.catalogue.key = la clave de lanzamiento de esta compilación no se puede leer.

journal.cli.sol.update.catalogue.lane = carril de lanzamiento desconocido: { $lane }

journal.cli.sol.update.catalogue.latest = el marcador del último lanzamiento está mal formado.

journal.cli.sol.update.catalogue.pin = el catálogo de lanzamientos indica una clave de lanzamiento distinta.

journal.cli.sol.update.catalogue.release = el catálogo de lanzamientos no coincide con el lanzamiento solicitado.

journal.cli.sol.update.catalogue.schema.bad.catalogue.revision = el catálogo de lanzamientos no es válido: revisión del catálogo incorrecta.

journal.cli.sol.update.catalogue.schema.bad.desktop.version = el catálogo de lanzamientos no es válido: versión de escritorio incorrecta.

journal.cli.sol.update.catalogue.schema.bad.installer = el catálogo de lanzamientos no es válido: instalador incorrecto.

journal.cli.sol.update.catalogue.schema.bad.version = el catálogo de lanzamientos no es válido: versión incorrecta.

journal.cli.sol.update.catalogue.schema.installer.in.an.unsupported.schema = el catálogo de lanzamientos no es válido: instalador en un esquema no compatible.

journal.cli.sol.update.catalogue.schema.key.id = el catálogo de lanzamientos no es válido: id de clave.

journal.cli.sol.update.catalogue.schema.no.journal.version = el catálogo de lanzamientos no es válido: sin versión del diario.

journal.cli.sol.update.catalogue.schema.no.version = el catálogo de lanzamientos no es válido: sin versión.

journal.cli.sol.update.catalogue.schema.not.json = el catálogo de lanzamientos no es válido: no es json.

journal.cli.sol.update.catalogue.schema.unsupported.protocol = el catálogo de lanzamientos no es válido: protocolo no compatible.

journal.cli.sol.update.catalogue.schema.unsupported.schema = el catálogo de lanzamientos no es válido: esquema no compatible.

journal.cli.sol.update.catalogue.signature = el catálogo de lanzamientos no pasó la revisión de la firma.

journal.cli.sol.update.catalogue.version = la versión de la plataforma solicitada no es válida.

journal.cli.sol.update.receipt.claimed.twice = componente { $component } reclamado por los recibos de tree y de paquete

journal.cli.sol.update.receipt.duplicate.roles = están registrados tanto el componente del diario como el de cli

journal.cli.sol.update.receipt.incomplete = el recibo de instalación está incompleto o no es compatible. usa el instalador original para recuperarlo.

journal.cli.sol.update.receipt.key.duplicate = clave duplicada { $key }

journal.cli.sol.update.receipt.key.outside = clave fuera de la sección

journal.cli.sol.update.receipt.line.invalid = línea mal formada: { $line }

journal.cli.sol.update.receipt.line.pair = no es key=value

journal.cli.sol.update.receipt.native.file = recibo nativo { $receipt_file } no es un archivo regular

journal.cli.sol.update.receipt.native.malformed = recibo nativo { $receipt_file } mal formado: { $detail }

journal.cli.sol.update.receipt.native.route = la ruta del recibo nativo debe ser tree

journal.cli.sol.update.receipt.native.schema = { $schema_field } del recibo nativo debe ser 1

journal.cli.sol.update.receipt.native.version = la versión instalada no coincide con { $journal_field } del recibo

journal.cli.sol.update.receipt.package.file = { $config_file } del paquete no es un archivo regular

journal.cli.sol.update.receipt.package.malformed = { $config_file } del paquete tiene un formato incorrecto: { $detail }

journal.cli.sol.update.receipt.section.duplicate = sección duplicada [{ $section }]

journal.cli.sol.update.receipt.section.key.duplicate = clave duplicada { $key } en la sección [{ $section }]

journal.cli.sol.update.receipt.section.unclosed = encabezado de sección sin cerrar

journal.cli.sol.update.receipt.section.unknown = sección no reconocida en { $config_file }: { $section }

journal.cli.sol.update.receipt.unrecorded = este diario se instaló desde un paquete sin el instalador de { $brand }, así que { $update_command } todavía no puede actualizarlo. ejecuta { $install_command } una vez. el comando revisa los archivos instalados contra el lanzamiento firmado, y { $update_command } lo actualiza después.

journal.cli.sol.update.receipt.user.file = { $config_file } de usuario no es un archivo regular

journal.cli.sol.update.receipt.user.malformed = { $config_file } de usuario tiene un formato incorrecto: { $detail }

journal.cli.sol.update.report.available = hay una actualización disponible.

journal.cli.sol.update.report.available.package.add = hay una actualización disponible. se instaló desde un paquete. para recibir actualizaciones con el actualizador de tu sistema, agrega el repositorio de { $brand }: { $package_url }

journal.cli.sol.update.report.available.package.configured = hay una actualización disponible. se instaló desde un paquete. las actualizaciones llegan con el actualizador de tu sistema.

journal.cli.sol.update.report.catalogue.invalid = los metadatos del lanzamiento seleccionado no son válidos.

journal.cli.sol.update.report.checkout = esta copia es una copia de trabajo de origen. instala el diario publicado para actualizarla.

journal.cli.sol.update.report.current = tu diario está al día.

journal.cli.sol.update.report.current.package.add = tu diario está al día. se instaló desde un paquete. para recibir actualizaciones con el actualizador de tu sistema, agrega el repositorio de { $brand }: { $package_url }

journal.cli.sol.update.report.current.package.configured = tu diario está al día. se instaló desde un paquete. las actualizaciones llegan con el actualizador de tu sistema.

journal.cli.sol.update.report.held = las actualizaciones están retenidas. ejecuta { $unhold_command } para liberarlas.

journal.cli.sol.update.report.hold.changed = no se pudo leer la retención de la actualización. no se cambió nada.

journal.cli.sol.update.report.hold.off = las actualizaciones ya no están retenidas.

journal.cli.sol.update.report.hold.on = las actualizaciones están retenidas.

journal.cli.sol.update.report.hold.save = no se pudo guardar la retención de la actualización. revisa los permisos de tu directorio de datos.

journal.cli.sol.update.report.hold.unreadable = no se pudo leer la retención de la actualización.

journal.cli.sol.update.report.install.directory = no se pudo identificar el directorio de instalación.

journal.cli.sol.update.report.installer.arguments = argumentos del instalador no válidos.

journal.cli.sol.update.report.installer.failed = la instalación no tuvo éxito. usa el instalador original para recuperar.

journal.cli.sol.update.report.installer.fetch = no se pudo obtener el instalador.

journal.cli.sol.update.report.installer.run = no se pudo ejecutar el instalador.

journal.cli.sol.update.report.installer.tookover = se actualizó tu diario. ahora se actualiza con el instalador de { $brand }, desde donde está.

journal.cli.sol.update.report.installer.updated = se actualizó tu diario.

journal.cli.sol.update.report.linux = { $update_command } está disponible en linux.

journal.cli.sol.update.report.mode = este modo no es una actualización.

journal.cli.sol.update.report.owner.unknown = no se pudo identificar cómo se instaló esta copia de { $brand }. usa el instalador que la instaló para actualizarla.

journal.cli.sol.update.report.platform.older = no se pudo hacer coincidir esta compilación anterior con un lanzamiento de la plataforma. elige una versión de la plataforma con { $version_flag }.

journal.cli.sol.update.report.rollback.package = esta ruta de paquete no puede instalar un diario anterior. no se cambió nada.

journal.cli.sol.update.report.unknown = el estado de la actualización es desconocido.

journal.cli.sol.update.rollback.epoch = instala la versión actual u otro lanzamiento en esta época de actualización

journal.cli.sol.update.rollback.link = falta el enlace de la versión actual o no se puede leer

journal.cli.sol.update.rollback.metadata = los metadatos del lanzamiento instalado no se pueden leer

journal.cli.sol.update.rollback.no.older = no se conserva ninguna compilación anterior elegible. elige una versión de la plataforma con { $version_flag }, o vuelve a instalar el lanzamiento actual.

journal.cli.sol.update.rollback.policy = el lanzamiento actual no tiene una política de retención compatible

journal.cli.sol.update.rollback.range.absent = el lanzamiento instalado no tiene un rango de lectura declarado. no se cambió nada.

journal.cli.sol.update.rollback.range.outside = la versión seleccionada está fuera del rango de lectura declarado. no se cambió nada.

journal.cli.sol.update.rollback.retained = esa compilación no se conserva; vuelve a instalar la versión actual

journal.cli.sol.update.rollback.state = el instalador no puede establecer el estado de tu diario. no se cambió nada.

journal.cli.sol.update.rollback.version = la versión de destino { $target_version } no es anterior a { $installed_version }

journal.cli.sol.update.summary.failure = { $code }: { $message }

journal.cli.sol.update.summary.hold.held = retención: retenida

journal.cli.sol.update.summary.hold.unheld = retención: no retenida

journal.cli.sol.update.summary.hold.unknown = retención: desconocida

journal.cli.sol.update.summary.journal = diario: { $value }

journal.cli.sol.update.summary.lane = carril: { $value }

journal.cli.sol.update.summary.platform = plataforma: { $value }

journal.cli.sol.update.summary.route = ruta: { $value }

journal.cli.sol.update.summary.target = disponible: { $available } (diario { $journal })

journal.cli.sol.usage = Uso: { $syntax }
