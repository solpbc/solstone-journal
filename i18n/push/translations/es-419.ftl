push.proof.count = { $count ->
    [one] { $count } elemento está listo.
    [many] { $count } elementos están listos.
   *[other] { $count } elementos están listos.
}

push.proof.new = un nuevo mensaje de prueba está listo.

push.proof.state = { $state ->
    [ready] listo para la prueba.
   *[waiting] en espera de la prueba.
}

push.test.body = notificación de prueba de tu diario.

push.test.title = solstone
