synth.notes.anchor = anchor-plain

synth.notes.label = label { $name }

synth.notes.gate = gate { $open }

synth.notes.span = { $count ->
    [one] span one { $count }
   *[other] span other { $count }
}

synth.notes.forms = { $count ->
    [one] forms one { $count }
   *[other] src-other { $count }
}

synth.notes.choice = { $kind ->
   *[box] kind-box
    [bag] kind-bag
}

synth.notes.group = group { $n }

synth.notes.wide = wide { $n }

synth.notes.shell = % \ " %n %1$s $NAME$ { $name }{ $gap }{ $cmd }{ $flag }{ $token }{ $link }{ $mark }

synth.notes.chrome = Chrome literal %n and %1$s slot { $var_name }

synth.notes.percent = %
