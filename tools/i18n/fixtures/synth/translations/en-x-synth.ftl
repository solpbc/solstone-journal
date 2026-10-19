synth.notes.plain = SYNTH A synthetic sentence.

synth.notes.named = SYNTH Hello { $name }.

synth.notes.tally = SYNTH Open { $open } count { $n }.

synth.notes.count = { $count ->
    [one] SYNTH one synthetic item
   *[other] { $count } SYNTH synthetic items
}

synth.notes.kind = { $kind ->
   *[box] SYNTH a synthetic box
    [bag] SYNTH a synthetic bag
}

synth.notes.pay = SYNTH Synthetic payment notice.

synth.notes.statute = SYNTH Synthetic statutory notice.

synth.notes.command = SYNTH Run{ $gap }{ $cmd }{ $flag } token { $token } link { $link } mark { $mark }.
