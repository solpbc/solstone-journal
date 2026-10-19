synth.notes.plain = A synthetic sentence.

synth.notes.named = Hello { $name }.

synth.notes.tally = Open { $open } count { $n }.

synth.notes.count = { $count ->
    [one] one synthetic item
   *[other] { $count } synthetic items
}

synth.notes.kind = { $kind ->
   *[box] a synthetic box
    [bag] a synthetic bag
}

synth.notes.pay = Synthetic payment notice.

synth.notes.statute = Synthetic statutory notice.

synth.notes.command = Run{ $gap }{ $cmd }{ $flag } token { $token } link { $link } mark { $mark }.
