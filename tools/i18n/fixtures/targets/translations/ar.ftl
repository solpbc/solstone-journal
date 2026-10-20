synth.notes.anchor = tr-plural-anchor

synth.notes.label = tr label { $name }

synth.notes.gate = tr gate { $open }

synth.notes.span = { $count ->
    [one] tr span one { $count }
   *[other] tr span other { $count }
}

synth.notes.forms = { $count ->
    [zero] forms zero { $count }
    [one] forms one { $count }
    [two] forms two { $count }
    [many] forms many { $count }
   *[other] tr-other { $count }
}

synth.notes.choice = { $kind ->
   *[box] kind-box
    [bag] kind-bag
}

synth.notes.group = tr group { $n }

synth.notes.wide = tr wide { $n }

synth.notes.shell = tr % \ " %n %1$s $NAME$ { $name }{ $gap }{ $cmd }{ $flag }{ $token }{ $link }{ $mark }

synth.notes.percent = %
