push.proof.count = { $count ->
    [one] { $count } elemento è pronto.
    [many] { $count } di elementi sono pronti.
   *[other] { $count } elementi sono pronti.
}

push.proof.new = un nuovo messaggio di prova è pronto.

push.proof.state = { $state ->
    [ready] pronto per la prova.
   *[waiting] in attesa della prova.
}

push.test.body = notifica di prova dal tuo diario.

push.test.title = solstone
