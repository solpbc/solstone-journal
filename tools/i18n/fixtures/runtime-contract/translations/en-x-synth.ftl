synth.notes.enumprobe = { $a-b ->
   *[Box] upper { $a-b } / { $a_b }
    [box] lower { $a-b } / { $a_b }
    [in-bag] hyphen { $a-b } / { $a_b }
    [in_bag] underscore { $a-b } / { $a_b }
}

synth.notes.countprobe = { $count ->
    [one] one { $count }
   *[other] other { $count }
}

synth.notes.mixedprobe = translated mixed { $n } / { $name } / { $open }

synth.notes.escapeprobe = { $gap }translated literal { $fmt } / { $name }{ $gap }

synth.notes.percentprobe = 100% %n %1$s $NAME$

synth.notes.spacesprobe = begin{ $gap }end / { $literal }
